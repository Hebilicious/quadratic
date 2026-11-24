use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::FIXED_ACTOR_NAMES;

pub const CREDENTIALS_FILE: &str = "credentials.json";
const VALUE_PREFIX: &str = "qdt_";
const RANDOM_BYTES: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum CredentialsError {
    #[error("{} cannot be read: {source}", path.display())]
    Unreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(
        "{} is not a credentials file ({problem}): restore a copy of it, or move it away to start with no tokens",
        path.display()
    )]
    Damaged { path: PathBuf, problem: String },
}

#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    #[error("the path of the token's file is relative")]
    NotAbsolute,
    #[error("the token's file would be in the server's data folder")]
    InDataFolder,
    #[error("a token has the label")]
    LabelTaken,
    #[error("something is already at the path of the token's file")]
    FileExists,
    #[error("the token cannot be created: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub label: String,
    pub created: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Verifier {
    label: String,
    sha256: String,
    created: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    tokens: Vec<Verifier>,
}

/// The live verifiers of agent tokens. A change writes a new `credentials.json` beside the old one and renames it over
/// the old one, under one lock. The rename is the commit: a failure before it changes neither the file nor memory;
/// once it succeeds, memory takes the change and the call succeeds, and a failed sync of the folder is only reported.
#[derive(Clone)]
pub struct Credentials {
    path: PathBuf,
    live: Arc<Mutex<Vec<Verifier>>>,
    disk: Disk,
    _held: Option<Arc<dyn Send + Sync>>,
}

/// The steps of a whole write of `credentials.json` that a test can make fail.
#[derive(Clone)]
struct Disk {
    rename: fn(&Path, &Path) -> std::io::Result<()>,
    sync_folder: fn(&Path) -> std::io::Result<()>,
    report: Report,
}

type Report = Arc<dyn Fn(&Path, &std::io::Error) + Send + Sync>;

impl Default for Disk {
    fn default() -> Disk {
        Disk {
            rename: |from, to| std::fs::rename(from, to),
            sync_folder: |folder| File::open(folder)?.sync_all(),
            report: Arc::new(|folder, error| {
                eprintln!(
                    "quadratic serve: {} is written, but its folder {} could not be synced ({error}); a crash now may lose the change",
                    CREDENTIALS_FILE,
                    folder.display()
                );
            }),
        }
    }
}

impl Credentials {
    pub fn open(path: PathBuf) -> Result<Credentials, CredentialsError> {
        let stored = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Stored>(&bytes).map_err(|error| {
                CredentialsError::Damaged {
                    path: path.clone(),
                    problem: error.to_string(),
                }
            })?,
            Err(error) if error.kind() == ErrorKind::NotFound => Stored::default(),
            Err(source) => return Err(CredentialsError::Unreadable { path, source }),
        };
        Ok(Credentials {
            path,
            live: Arc::new(Mutex::new(stored.tokens)),
            disk: Disk::default(),
            _held: None,
        })
    }

    /// Keeps `held` until every copy is gone, the last one in a write that a shutdown left running, so the server's
    /// home lock outlives any write of `credentials.json`.
    pub fn holding(self, held: impl Send + Sync + 'static) -> Credentials {
        Credentials {
            _held: Some(Arc::new(held)),
            ..self
        }
    }

    pub fn create(&self, label: &str, file: &Path) -> Result<Token, CreateError> {
        if !file.is_absolute() {
            return Err(CreateError::NotAbsolute);
        }
        if self.in_data_folder(file) {
            return Err(CreateError::InDataFolder);
        }
        let mut live = self.live();
        if FIXED_ACTOR_NAMES.contains(&label) || live.iter().any(|verifier| verifier.label == label)
        {
            return Err(CreateError::LabelTaken);
        }
        let value = new_value()?;
        let mut holder = match new_private_file(file) {
            Ok(holder) => holder,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                return Err(CreateError::FileExists);
            }
            Err(error) => return Err(naming(file, error).into()),
        };
        let token = Token {
            label: label.to_owned(),
            created: jiff::Timestamp::now()
                .strftime("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
        };
        let mut tokens = live.clone();
        tokens.push(Verifier {
            label: token.label.clone(),
            sha256: digest(&value),
            created: token.created.clone(),
        });
        let written = write_all_synced(&mut holder, value.as_bytes())
            .map_err(|error| naming(file, error))
            .and_then(|()| self.write_whole(&tokens));
        if let Err(error) = written {
            drop(holder);
            let _ = std::fs::remove_file(file);
            return Err(error.into());
        }
        *live = tokens;
        Ok(token)
    }

    pub fn list(&self) -> Vec<Token> {
        self.live()
            .iter()
            .map(|verifier| Token {
                label: verifier.label.clone(),
                created: verifier.created.clone(),
            })
            .collect()
    }

    /// Whether a token had the label. A label that no token has changes nothing, not even the file.
    pub fn revoke(&self, label: &str) -> std::io::Result<bool> {
        let mut live = self.live();
        let tokens: Vec<Verifier> = live
            .iter()
            .filter(|verifier| verifier.label != label)
            .cloned()
            .collect();
        if tokens.len() == live.len() {
            return Ok(false);
        }
        self.write_whole(&tokens)?;
        *live = tokens;
        Ok(true)
    }

    /// The label of the live token whose value this is, from memory alone.
    pub fn label_of(&self, value: &str) -> Option<String> {
        let wanted = digest(value);
        self.live()
            .iter()
            .find(|verifier| verifier.sha256 == wanted)
            .map(|verifier| verifier.label.clone())
    }

    fn in_data_folder(&self, file: &Path) -> bool {
        let real = |folder: Option<&Path>| folder.and_then(|folder| folder.canonicalize().ok());
        let data_folder = real(self.path.parent());
        data_folder.is_some() && real(file.parent()) == data_folder
    }

    fn write_whole(&self, tokens: &[Verifier]) -> std::io::Result<()> {
        let partial = self.path.with_extension("json.partial");
        prepare(&partial, tokens)
            .and_then(|()| (self.disk.rename)(&partial, &self.path))
            .map_err(|error| naming(&self.path, error))?;
        if let Some(folder) = self.path.parent()
            && let Err(error) = (self.disk.sync_folder)(folder)
        {
            (self.disk.report)(folder, &error);
        }
        Ok(())
    }

    fn live(&self) -> MutexGuard<'_, Vec<Verifier>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn new_value() -> std::io::Result<String> {
    let mut bytes = [0_u8; RANDOM_BYTES];
    let mut filled = 0;
    while filled < bytes.len() {
        filled +=
            rustix::rand::getrandom(&mut bytes[filled..], rustix::rand::GetRandomFlags::empty())
                .map_err(std::io::Error::from)?;
    }
    Ok(format!("{VALUE_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn naming(path: &Path, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

fn prepare(partial: &Path, tokens: &[Verifier]) -> std::io::Result<()> {
    let text = serde_json::to_vec_pretty(&Stored {
        tokens: tokens.to_vec(),
    })?;
    match std::fs::remove_file(partial) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = new_private_file(partial)?;
    write_all_synced(&mut file, &text)
}

fn write_all_synced(file: &mut File, bytes: &[u8]) -> std::io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
}

/// `create_new` is `O_CREAT | O_EXCL`: it fails on anything already at the path, a dangling symbolic link included.
fn new_private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials(home: &Path) -> Credentials {
        let data_folder = home.join("data");
        std::fs::create_dir(&data_folder).expect("a data folder");
        Credentials::open(data_folder.join(CREDENTIALS_FILE)).expect("the credentials open")
    }

    fn text(path: &Path) -> String {
        path.display().to_string()
    }

    fn value_in(file: &Path) -> String {
        std::fs::read_to_string(file).expect("the token's file")
    }

    #[test]
    fn a_value_is_the_prefix_and_32_random_bytes_in_base64url() {
        let value = new_value().expect("a value");
        let encoded = value.strip_prefix(VALUE_PREFIX).expect("the prefix");
        let bytes = URL_SAFE_NO_PAD.decode(encoded).expect("base64url");
        assert_eq!(bytes.len(), RANDOM_BYTES);
        assert_ne!(new_value().expect("a value"), value);
    }

    #[test]
    fn a_value_that_no_live_token_has_answers_no_label() {
        let home = tempfile::tempdir().expect("a home");
        let file = home.path().join("token");
        let credentials = credentials(home.path());
        credentials.create("deploy", &file).expect("created");
        let other = format!("{}x", value_in(&file));
        assert_eq!(credentials.label_of(&other), None);
        assert_eq!(credentials.label_of(""), None);
    }

    fn failing(_: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other("the disk refused"))
    }

    fn failing_rename(_: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other("the disk refused"))
    }

    fn stored_labels(credentials: &Credentials) -> Vec<String> {
        let bytes = std::fs::read(&credentials.path).expect("credentials.json");
        let stored: Stored = serde_json::from_slice(&bytes).expect("a credentials file");
        stored
            .tokens
            .into_iter()
            .map(|verifier| verifier.label)
            .collect()
    }

    fn live_labels(credentials: &Credentials) -> Vec<String> {
        credentials
            .list()
            .into_iter()
            .map(|token| token.label)
            .collect()
    }

    #[test]
    fn a_failed_rename_changes_nothing() {
        let home = tempfile::tempdir().expect("a home");
        let mut credentials = credentials(home.path());
        credentials
            .create("kept", &home.path().join("kept"))
            .expect("created");
        credentials.disk.rename = failing_rename;
        let file = home.path().join("token");

        let created = credentials.create("deploy", &file);
        let revoked = credentials.revoke("kept");

        let Err(CreateError::Io(error)) = created else {
            panic!("not an input-output error: {created:?}");
        };
        assert!(
            error.to_string().contains(&text(&credentials.path)),
            "{error}"
        );
        assert!(revoked.is_err(), "{revoked:?}");
        assert!(
            !file.exists(),
            "the token's file stayed after a failed create"
        );
        assert_eq!(live_labels(&credentials), ["kept"]);
        assert_eq!(stored_labels(&credentials), ["kept"]);
    }

    #[test]
    fn after_the_rename_a_failed_folder_sync_is_reported_and_the_change_holds() {
        let home = tempfile::tempdir().expect("a home");
        let mut credentials = credentials(home.path());
        credentials
            .create("kept", &home.path().join("kept"))
            .expect("created");
        let reports = Arc::new(Mutex::new(Vec::new()));
        credentials.disk.sync_folder = failing;
        let seen = Arc::clone(&reports);
        credentials.disk.report = Arc::new(move |folder: &Path, error: &std::io::Error| {
            seen.lock()
                .expect("the reports")
                .push(format!("{}: {error}", folder.display()));
        });
        let file = home.path().join("token");

        let created = credentials.create("deploy", &file);
        let revoked = credentials.revoke("kept");

        assert!(created.is_ok(), "{created:?}");
        assert!(matches!(revoked, Ok(true)), "{revoked:?}");
        assert!(
            file.exists(),
            "the token's file went after a committed create"
        );
        assert_eq!(live_labels(&credentials), ["deploy"]);
        assert_eq!(stored_labels(&credentials), ["deploy"]);
        assert_eq!(reports.lock().expect("the reports").len(), 2);
    }
}
