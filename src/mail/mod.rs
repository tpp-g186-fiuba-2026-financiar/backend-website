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
