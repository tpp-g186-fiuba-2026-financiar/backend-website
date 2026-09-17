use lettre::{
    message::header::ContentType, transport::smtp::authentication::Credentials, AsyncSmtpTransport,
    AsyncTransport, Message, Tokio1Executor,
};

/// Configuracion del proveedor SMTP para las alertas de tendencia. Ausente
/// (`from_env` devuelve `None`) cuando no se seteo `SMTP_HOST`: el job de
/// alertas sigue corriendo y logueando cambios de tendencia, solo que no
/// manda mails. Asi el resto del equipo puede levantar el backend sin
/// necesidad de credenciales de mail.
#[derive(Clone)]
pub struct MailConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from_email: String,
    pub from_name: String,
}

impl MailConfig {
    pub fn from_env() -> Option<Self> {
        let host = std::env::var("SMTP_HOST").ok()?;
        let port = std::env::var("SMTP_PORT")
            .unwrap_or_else(|_| "587".to_string())
            .parse()
            .unwrap_or(587);
        let username = std::env::var("SMTP_USERNAME").unwrap_or_default();
        let password = std::env::var("SMTP_PASSWORD").unwrap_or_default();
        let from_email =
            std::env::var("SMTP_FROM_EMAIL").unwrap_or_else(|_| "alertas@financiar.app".into());
        let from_name = std::env::var("SMTP_FROM_NAME").unwrap_or_else(|_| "FinanciAr".into());

        Some(Self {
            host,
            port,
            username,
            password,
            from_email,
            from_name,
        })
    }

    fn transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
        let creds = Credentials::new(self.username.clone(), self.password.clone());
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.host)
            .map_err(|err| format!("No se pudo construir el transporte SMTP: {err}"))
            .map(|builder| builder.port(self.port).credentials(creds).build())
    }
}

/// Contenido de una alerta de cambio de tendencia para un ticker puntual.
pub struct TrendAlert<'a> {
    pub ticker: &'a str,
    pub previous_condition: &'a str,
    pub new_condition: &'a str,
    pub signal: Option<&'a str>,
    pub as_of: Option<&'a str>,
}

pub async fn send_trend_alert(
    config: &MailConfig,
    to_email: &str,
    to_name: &str,
    alert: &TrendAlert<'_>,
) -> Result<(), String> {
    let subject = format!("FinanciAr: {} cambio de tendencia", alert.ticker);
    let signal_line = alert
        .signal
        .map(|signal| format!("Senal actual: {signal}.\n"))
        .unwrap_or_default();
    let as_of_line = alert
        .as_of
        .map(|as_of| format!("Ultima rueda considerada: {as_of}.\n"))
        .unwrap_or_default();
    let body = format!(
        "Hola {to_name},\n\n\
        {ticker} paso de {previous} a {new}.\n\
        {signal_line}{as_of_line}\n\
        Podes ver el detalle actualizado ingresando a tu cuenta de FinanciAr.\n\n\
        Este mail se envio porque estas suscripto a alertas de este ticker o de tu cartera.",
        ticker = alert.ticker,
        previous = alert.previous_condition,
        new = alert.new_condition,
    );

    let email = Message::builder()
        .from(
            format!("{} <{}>", config.from_name, config.from_email)
                .parse()
                .map_err(|err| format!("From invalido: {err}"))?,
        )
        .to(format!("{to_name} <{to_email}>")
            .parse()
            .map_err(|err| format!("Destinatario invalido: {err}"))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|err| format!("No se pudo armar el mail: {err}"))?;

    let transport = config.transport()?;
    transport
        .send(email)
        .await
        .map(|_| ())
        .map_err(|err| format!("Fallo el envio SMTP: {err}"))
}

/// Una linea de tenencia dentro del reporte semanal (ver `src::reports`).
pub struct WeeklyReportLine<'a> {
    pub ticker: &'a str,
    pub quantity: i32,
    pub current_price: f64,
    /// None cuando no habia cotizacion de hace 7 dias (ticker recien
    /// incorporado al catalogo, feriado largo, etc.) -- la linea se manda
    /// igual, solo sin el dato de variacion semanal.
    pub week_change_pct: Option<f64>,
    /// None cuando la tenencia no tiene precio de entrada cargado.
    pub pnl_pct: Option<f64>,
}

pub struct WeeklyReport<'a> {
    pub lines: &'a [WeeklyReportLine<'a>],
    pub total_current_value: f64,
    /// None cuando ninguna tenencia tiene variacion semanal resuelta.
    pub total_week_change_pct: Option<f64>,
}

pub async fn send_weekly_report(
    config: &MailConfig,
    to_email: &str,
    to_name: &str,
    report: &WeeklyReport<'_>,
) -> Result<(), String> {
    let subject = "FinanciAr: tu resumen semanal de inversiones".to_string();

    let mut lines_text = String::new();
    for line in report.lines {
        let change = match line.week_change_pct {
            Some(pct) => format!("{:+.1}% en la semana", pct),
            None => "sin variacion semanal disponible".to_string(),
        };
        let pnl = match line.pnl_pct {
            Some(pct) => format!(", {:+.1}% desde tu precio de entrada", pct),
            None => String::new(),
        };
        lines_text.push_str(&format!(
            "- {ticker} x{quantity}: {price:.2} ({change}{pnl})\n",
            ticker = line.ticker,
            quantity = line.quantity,
            price = line.current_price,
        ));
    }

    let total_change_line = match report.total_week_change_pct {
        Some(pct) => format!("Tu cartera se movio {pct:+.1}% esta semana.\n"),
        None => String::new(),
    };

    let body = format!(
        "Hola {to_name},\n\n\
        Asi viene tu cartera esta semana:\n\n\
        {lines_text}\n\
        Valor total actual: {total_value:.2}.\n\
        {total_change_line}\n\
        Podes ver el detalle actualizado ingresando a tu cuenta de FinanciAr.\n\n\
        Este mail se envia una vez por semana porque tenes acciones cargadas en tu cartera.",
        total_value = report.total_current_value,
    );

    let email = Message::builder()
        .from(
            format!("{} <{}>", config.from_name, config.from_email)
                .parse()
                .map_err(|err| format!("From invalido: {err}"))?,
        )
        .to(format!("{to_name} <{to_email}>")
            .parse()
            .map_err(|err| format!("Destinatario invalido: {err}"))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|err| format!("No se pudo armar el mail: {err}"))?;

    let transport = config.transport()?;
    transport
        .send(email)
        .await
        .map(|_| ())
        .map_err(|err| format!("Fallo el envio SMTP: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_config_reads_defaults_and_explicit_values() {
        std::env::remove_var("SMTP_HOST");
        assert!(MailConfig::from_env().is_none());

        std::env::set_var("SMTP_HOST", "smtp.example.test");
        std::env::set_var("SMTP_PORT", "invalid");
        std::env::remove_var("SMTP_USERNAME");
        std::env::remove_var("SMTP_PASSWORD");
        std::env::remove_var("SMTP_FROM_EMAIL");
        std::env::remove_var("SMTP_FROM_NAME");
        let defaults = MailConfig::from_env().unwrap();
        assert_eq!(defaults.port, 587);
        assert_eq!(defaults.from_email, "alertas@financiar.app");
        assert_eq!(defaults.from_name, "FinanciAr");

        std::env::set_var("SMTP_PORT", "2525");
        std::env::set_var("SMTP_USERNAME", "mailer");
        std::env::set_var("SMTP_PASSWORD", "secret");
        std::env::set_var("SMTP_FROM_EMAIL", "mail@example.test");
        std::env::set_var("SMTP_FROM_NAME", "Coverage");
        let explicit = MailConfig::from_env().unwrap();
        assert_eq!(explicit.port, 2525);
        assert_eq!(explicit.username, "mailer");
        assert_eq!(explicit.password, "secret");
        assert_eq!(explicit.from_email, "mail@example.test");
        assert_eq!(explicit.from_name, "Coverage");

        std::env::remove_var("SMTP_HOST");
    }
}
