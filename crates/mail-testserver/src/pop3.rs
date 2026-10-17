//! The POP3 side (RFC 1939): the account's inbox as a maildrop. A session sees the inbox as it
//! was when it began (UIDL `uid-<UID>`); the messages it removed with DELE leave the inbox at
//! QUIT, as on a real server.

use std::sync::Arc;

use mail_core::pop3::fake::FakePop3;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{Io, Shared};

pub(crate) async fn session(io: Io, shared: Arc<Shared>) -> std::io::Result<()> {
    let mut fake = FakePop3::new(&shared.user, &shared.password.lock().unwrap_or_else(|e| e.into_inner()));
    {
        let f = shared.fake.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(inbox) = f.mailboxes.get("INBOX") {
            for (uid, m) in &inbox.messages {
                if !m.flags.deleted {
                    fake.messages.push((format!("uid-{uid}"), m.raw.clone(), false));
                }
            }
        }
    }
    let before: Vec<String> = fake.messages.iter().map(|m| m.0.clone()).collect();
    *shared.pop3_sessions.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    let mut io = BufReader::new(io);
    io.get_mut().write_all(fake.greeting().as_bytes()).await?;
    loop {
        let mut line = String::new();
        if io.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let (answer, bye) = fake.handle(&line);
        io.get_mut().write_all(answer.as_bytes()).await?;
        io.get_mut().flush().await?;
        if bye {
            break;
        }
    }
    let after: Vec<String> = fake.messages.iter().map(|m| m.0.clone()).collect();
    let gone: Vec<u32> =
        before.iter().filter(|u| !after.contains(u)).filter_map(|u| u.strip_prefix("uid-")?.parse().ok()).collect();
    if !gone.is_empty() {
        shared.with(|f| {
            for uid in gone {
                f.expunge("INBOX", uid);
            }
        });
    }
    Ok(())
}
