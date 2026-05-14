use chrono::{Duration, Utc};
use jsonwebtoken::{
    decode, encode, errors::Error as JwtError, DecodingKey, EncodingKey, Header, Validation,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: i32,
    pub email: String,
    pub exp: usize,
    pub iat: usize,
}

#[derive(Clone)]
pub struct JwtConfig {
    secret: String,
    expiration_hours: i64,
}

impl JwtConfig {
    pub fn from_env() -> Self {
        let secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
        let expiration_hours = std::env::var("JWT_EXPIRATION_HOURS")
            .unwrap_or_else(|_| "24".to_string())
            .parse()
            .expect("JWT_EXPIRATION_HOURS must be a valid number");
        Self {
            secret,
            expiration_hours,
        }
    }

    pub fn new(secret: impl Into<String>, expiration_hours: i64) -> Self {
        Self {
            secret: secret.into(),
            expiration_hours,
        }
    }

    pub fn encode_token(&self, user_id: i32, email: &str) -> Result<String, JwtError> {
        let now = Utc::now();
        let exp = now + Duration::hours(self.expiration_hours);
        let claims = Claims {
            sub: user_id,
            email: email.to_string(),
            exp: exp.timestamp() as usize,
            iat: now.timestamp() as usize,
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )
    }

    pub fn decode_token(&self, token: &str) -> Result<Claims, JwtError> {
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &Validation::default(),
        )?;
        Ok(data.claims)
    }
}
