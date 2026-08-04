use axum::response::Html;

/// Sirve el HTML del tablero. Embebido en el binario en tiempo de
/// compilacion (no hay servido de archivos estaticos en este proyecto
/// todavia, no vale la pena sumar esa dependencia por un solo archivo).
pub async fn handler() -> Html<&'static str> {
    Html(include_str!("../../../static/retro.html"))
}
