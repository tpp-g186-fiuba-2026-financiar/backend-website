use regex::Regex;

use crate::endpoints::user::registration::{
    registration_logic::RegisterUserRequest, validators::Validator,
};

pub struct EmailValidator {
    re_mail: Regex,
}
impl EmailValidator {
    pub fn new() -> Self {
        EmailValidator {
            re_mail: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b").unwrap(),
        }
    }
}

impl Default for EmailValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl Validator for EmailValidator {
    fn validate(&self, text: &RegisterUserRequest) -> Result<(), String> {
        let text = text.email.trim();
        if self.re_mail.is_match(text) {
            Ok(())
        } else {
            Err("Invalid email format".to_string())
        }
    }
}
