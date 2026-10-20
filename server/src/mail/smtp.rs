//! Sending through an SMTP server (lettre): a mail service's relay, or one
//! of your own. A connection per mail, which is plenty for sign-in codes.

use std::time::Duration;

use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use super::{Mail, Sender};
use crate::error::{Error, Result};

/// How the connection is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// Plain at first, then STARTTLS, which the server must offer. Port 587.
    StartTls,
    /// TLS from the first byte (SMTPS). Port 465.
    Tls,
    /// None at all: only for a relay on this machine or a private network.
    None,
}

#[derive(Clone, PartialEq)]
pub struct Config {
    /// FUWA_SMTP_HOST.
    pub host: String,
    /// FUWA_SMTP_PORT, default 587 (465 for tls, 25 for none).
    pub port: u16,
    /// FUWA_SMTP_SECURITY: starttls (default), tls or none.
    pub security: Security,
    /// FUWA_SMTP_USERNAME and FUWA_SMTP_PASSWORD, when the server wants them.
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Config {
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> std::result::Result<Self, String> {
        let host = get("FUWA_SMTP_HOST").map(|host| host.trim().to_string()).unwrap_or_default();
        if host.is_empty() {
            return Err("FUWA_MAIL_PROVIDER=smtp needs FUWA_SMTP_HOST".into());
        }
        let security = match get("FUWA_SMTP_SECURITY").as_deref().map(str::trim) {
            None | Some("starttls") => Security::StartTls,
            Some("tls") => Security::Tls,
            Some("none") => Security::None,
            Some(other) => return Err(format!("FUWA_SMTP_SECURITY must be starttls, tls or none, got {other:?}")),
        };
        let port = match get("FUWA_SMTP_PORT") {
            Some(port) => port
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| format!("FUWA_SMTP_PORT must be a port number, got {port:?}"))?,
            None => match security {
                Security::StartTls => 587,
                Security::Tls => 465,
                Security::None => 25,
            },
        };
        let username = get("FUWA_SMTP_USERNAME").map(|user| user.trim().to_string()).filter(|user| !user.is_empty());
        let password = get("FUWA_SMTP_PASSWORD").filter(|password| !password.is_empty());
        if username.is_some() != password.is_some() {
            return Err("FUWA_SMTP_USERNAME and FUWA_SMTP_PASSWORD go together".into());
        }
        Ok(Self { host, port, security, username, password })
    }
}

pub struct Smtp {
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl Smtp {
    pub fn new(config: &Config) -> Result<Self> {
        let tls = || TlsParameters::new(config.host.clone()).map_err(|err| Error::internal(format!("mail: {err}")));
        let tls = match config.security {
            Security::StartTls => Tls::Required(tls()?),
            Security::Tls => Tls::Wrapper(tls()?),
            Security::None => Tls::None,
        };
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.host)
            .port(config.port)
            .tls(tls)
            .timeout(Some(Duration::from_secs(30)));
        if let (Some(username), Some(password)) = (&config.username, &config.password) {
            builder = builder.credentials(Credentials::new(username.clone(), password.clone()));
        }
        Ok(Self { transport: builder.build() })
    }
}

/// The message as it goes over the wire: plain text and HTML as alternatives.
fn message(from: &str, mail: &Mail) -> Result<Message> {
    let from: Mailbox = from.parse().map_err(|err| Error::internal(format!("mail: FUWA_MAIL_FROM: {err}")))?;
    let to: Mailbox = mail.to.parse().map_err(|_| Error::invalid("that doesn't look like an email address"))?;
    Message::builder()
        .from(from)
        .to(to)
        .subject(&mail.subject)
        .multipart(MultiPart::alternative_plain_html(mail.text.clone(), mail.html.clone()))
        .map_err(|err| Error::internal(format!("mail: {err}")))
}

#[tonic::async_trait]
impl Sender for Smtp {
    async fn send(&self, from: &str, mail: &Mail) -> Result<()> {
        let message = message(from, mail)?;
        self.transport.send(message).await.map_err(|err| Error::Unavailable(format!("the mail server: {err}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::*;

    /// Plays an SMTP server for one mail, and returns what it was told.
    async fn fake_server() -> (u16, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let serving = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();
            let mut heard = Vec::new();
            write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
            let mut in_data = false;
            while let Some(line) = lines.next_line().await.unwrap() {
                heard.push(line.clone());
                let answer: &[u8] = if in_data {
                    if line != "." {
                        continue;
                    }
                    in_data = false;
                    b"250 queued\r\n"
                } else if line.starts_with("EHLO") {
                    b"250-fake\r\n250 8BITMIME\r\n"
                } else if line == "DATA" {
                    in_data = true;
                    b"354 go on\r\n"
                } else if line == "QUIT" {
                    write.write_all(b"221 bye\r\n").await.unwrap();
                    break;
                } else {
                    b"250 ok\r\n"
                };
                write.write_all(answer).await.unwrap();
            }
            heard
        });
        (port, serving)
    }

    #[tokio::test]
    async fn sends_one_mail_over_smtp() {
        let (port, serving) = fake_server().await;
        let config =
            Config { host: "127.0.0.1".into(), port, security: Security::None, username: None, password: None };
        let mail = Mail {
            to: "ana@example.com".into(),
            subject: "123456 is your code".into(),
            text: "Your code is 123456".into(),
            html: "<p>Your code is <b>123456</b></p>".into(),
        };
        Smtp::new(&config).unwrap().send("fuwa <no-reply@fuwa.chat>", &mail).await.unwrap();
        let heard = serving.await.unwrap();
        let heard = heard.join("\n");
        assert!(heard.contains("MAIL FROM:<no-reply@fuwa.chat>"), "{heard}");
        assert!(heard.contains("RCPT TO:<ana@example.com>"), "{heard}");
        assert!(heard.contains("Subject: 123456 is your code"), "{heard}");
        assert!(heard.contains("multipart/alternative"), "{heard}");
        assert!(heard.contains("Your code is 123456"), "{heard}");
    }

    #[test]
    fn ports_follow_security() {
        let vars = |security: &'static str| {
            move |key: &str| match key {
                "FUWA_SMTP_HOST" => Some("smtp.example.com".to_string()),
                "FUWA_SMTP_SECURITY" => Some(security.to_string()),
                _ => None,
            }
        };
        assert_eq!(Config::from_lookup(&vars("starttls")).unwrap().port, 587);
        assert_eq!(Config::from_lookup(&vars("tls")).unwrap().port, 465);
        assert_eq!(Config::from_lookup(&vars("none")).unwrap().port, 25);
        assert!(Config::from_lookup(&vars("ssl")).is_err());
    }
}
