//! Org SMTP settings and outbound mail (user invites).
use lettre::message::{Mailbox, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use pdm_core::PdmError;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct MailSettings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
    pub tls: String,
    pub login_url: String,
}

impl MailSettings {
    pub fn from_json(v: &Value) -> Self {
        let port = v.get("port").and_then(Value::as_u64).unwrap_or(587);
        let port = u16::try_from(port).unwrap_or(587);
        Self {
            host: str_field(v, "host"),
            port,
            username: str_field(v, "username"),
            password: str_field(v, "password"),
            from: str_field(v, "from"),
            tls: {
                let t = str_field(v, "tls");
                if t.is_empty() { "starttls".into() } else { t }
            },
            login_url: str_field(v, "login_url"),
        }
    }

    pub fn configured(&self) -> bool {
        !self.host.trim().is_empty() && !self.from.trim().is_empty()
    }

    pub fn public(&self) -> Value {
        json!({
            "host": self.host,
            "port": self.port,
            "username": self.username,
            "from": self.from,
            "tls": self.tls,
            "login_url": self.login_url,
            "password_set": !self.password.is_empty(),
            "configured": self.configured(),
        })
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

pub async fn send_mail(
    settings: &MailSettings,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<(), PdmError> {
    if !settings.configured() {
        return Err(PdmError::BadRequest(
            "mail server is not configured — set it in Settings".into(),
        ));
    }
    let from: Mailbox = settings
        .from
        .parse()
        .map_err(|e| PdmError::BadRequest(format!("mail from address: {e}")))?;
    let to_box: Mailbox = to
        .parse()
        .map_err(|e| PdmError::BadRequest(format!("mail to address: {e}")))?;
    let email = Message::builder()
        .from(from)
        .to(to_box)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| PdmError::Internal(format!("mail message: {e}")))?;

    let mut builder = match settings.tls.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.host)
            .map_err(|e| PdmError::BadRequest(format!("smtp tls: {e}")))?,
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.host),
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.host)
            .map_err(|e| PdmError::BadRequest(format!("smtp starttls: {e}")))?,
    };
    builder = builder.port(settings.port);
    if !settings.username.is_empty() {
        builder = builder.credentials(Credentials::new(
            settings.username.clone(),
            settings.password.clone(),
        ));
    }
    let transport = builder.build();
    transport
        .send(email)
        .await
        .map_err(|e| PdmError::Internal(format!("smtp send: {e}")))?;
    Ok(())
}

pub fn invite_body(settings: &MailSettings, email: &str, password: &str, org_name: &str) -> String {
    let url = if settings.login_url.trim().is_empty() {
        "the Foundry PDM web workspace".to_string()
    } else {
        settings.login_url.trim().to_string()
    };
    format!(
        "You have been invited to {org_name} on Foundry PDM.\n\n\
Sign in at {url}\n\
Email: {email}\n\
Temporary password: {password}\n\n\
Change this password after you sign in.\n"
    )
}

pub fn random_password() -> String {
    use rand::Rng;
    const C: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rng();
    (0..12)
        .map(|_| C[rng.random_range(0..C.len())] as char)
        .collect()
}
