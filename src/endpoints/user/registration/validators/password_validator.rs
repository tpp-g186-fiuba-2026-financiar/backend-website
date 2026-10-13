use crate::endpoints::user::registration::registration_logic::RegisterUserRequest;

pub struct PasswordValidator;

impl super::Validator for PasswordValidator {
    fn validate(&self, payload: &RegisterUserRequest) -> Result<(), String> {
        validate_password(&payload.password)
    }
}

/// Reglas de contrasena compartidas entre el registro y el reseteo de
/// contrasena (`password_reset::reset_logic`).
pub fn validate_password(password: &str) -> Result<(), String> {
    let text = password.trim();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_strong_password() {
        assert!(validate_password("StrongPassword123!").is_ok());
    }

    #[test]
    fn rejects_passwords_missing_a_requirement() {
        assert!(validate_password("Sh0rt!").is_err());
        assert!(validate_password("alllowercase123!").is_err());
        assert!(validate_password("ALLUPPERCASE123!").is_err());
        assert!(validate_password("NoDigitsHere!!").is_err());
        assert!(validate_password("NoSpecials1234").is_err());
    }
}
