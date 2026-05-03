use crate::endpoints::user::registration::registration_logic::RegisterUserRequest;

pub mod email_validator;
pub mod password_validator;

pub trait Validator {
    fn validate(&self, payload: &RegisterUserRequest) -> Result<(), String>;
}
