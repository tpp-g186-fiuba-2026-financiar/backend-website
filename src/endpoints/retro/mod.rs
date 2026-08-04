//! Tablero de retro del equipo. No es parte del dominio del TP: es una
//! herramienta interna, sin autenticacion de usuario (las tarjetas son
//! anonimas a proposito), protegida solo por un PIN compartido (`pin.rs`)
//! porque el repo es publico. A proposito no esta en el `ApiDoc` de
//! `utoipa`/Swagger de `lib.rs`.
pub mod archive_logic;
pub mod board_logic;
pub mod cards_logic;
pub mod page;
mod pin;
mod sprint;
