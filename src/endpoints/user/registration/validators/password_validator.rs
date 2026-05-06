use crate::endpoints::user::registration::registration_logic::RegisterUserRequest;

pub struct PasswordValidator;

impl super::Validator for PasswordValidator {
    fn validate(&self, payload: &RegisterUserRequest) -> Result<(), String> {
        let text = payload.password.trim();

        if text.len() < 8 {
            return Err("Password must be at least 8 characters long".to_string());
        }
        if !text.chars().any(|c| c.is_uppercase()) {
            return Err("Password must contain at least one uppercase letter".to_string());
        }
        if !text.chars().any(|c| c.is_lowercase()) {
            return Err("Password must contain at least one lowercase letter".to_string());
        }
        if !text.chars().any(|c| c.is_ascii_digit()) {
            return Err("Password must contain at least one digit".to_string());
        }

        // Check for special characters
        let special_characters = r#"!@#$%^&*()_+-=[]{}|;':\",.<>/?"#;
        if !text.chars().any(|c| special_characters.contains(c)) {
            return Err("Password must contain at least one special character".to_string());
        }
        Ok(())
    }
}
