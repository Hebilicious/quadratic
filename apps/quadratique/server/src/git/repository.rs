use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::environment::{self, Configuration};
use super::ids;
use super::watch::{Commit, Head, Place, Reached, Reading, Seen};
use crate::store::GitWatch;

const HEADS: &str = "refs/heads/";
const REMOTES: &str = "refs/remotes/";
const REFS: &str = "--format=%(objectname) %(refname) %(upstream)";
const NOT_AN_ANCESTOR: i32 = 1;
const NOT_A_COMMIT: i32 = 1;

pub const GIT_LIMIT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git cannot start: {0}")]
    Start(#[from] std::io::Error),
    #[error("git {command} fails in {repository}: {message}")]
    Failed {
        command: String,
        repository: String,
        message: String,
    },
    #[error("git {command} in {repository} did not end within {limit:?} and was stopped")]
    TooSlow {
        command: String,
        repository: String,
        limit: Duration,
    },
    #[error("git {0} printed text that is not UTF-8")]
    NotText(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingTips {
    pub local: Option<String>,
    pub upstream: Option<String>,
}

struct Output {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub struct Repository {
    git_dir: String,
    limit: Duration,
    configuration: Configuration,
}

impl Repository {
    pub fn at(git_dir: &str, limit: Duration) -> Repository {
        Repository {
            git_dir: git_dir.to_owned(),
            limit,
            configuration: Configuration::User,
        }
    }

    /// The same repository, read with `path` as git's global configuration and no system one.
    pub fn with_global_configuration(self, path: PathBuf) -> Repository {
        Repository {
            configuration: Configuration::Global(path),
            ..self
        }
    }

    pub async fn heads(&self) -> Result<String, GitError> {
        self.text(&["for-each-ref", REFS, HEADS, REMOTES]).await
    }

    /// Every local branch and the commit it points to, by its exact name.
    pub async fn branch_tips(&self) -> Result<BTreeMap<String, String>, GitError> {
        let text = self.text(&["for-each-ref", REFS, HEADS]).await?;
        Ok(parse_heads(&text))
    }

    pub async fn tip(&self, branch: &str) -> Result<Option<String>, GitError> {
        let reference = format!("{HEADS}{branch}");
        let text = self.text(&["for-each-ref", REFS, &reference]).await?;
        Ok(parse_heads(&text).remove(branch))
    }

    pub async fn look(
        &self,
        default_branch: &str,
        known: Option<&GitWatch>,
    ) -> Result<Option<Seen>, GitError> {
        let refs = self.heads().await?;
        self.look_at(&refs, default_branch, known).await
    }

    pub async fn look_at(
        &self,
        refs: &str,
        default_branch: &str,
        known: Option<&GitWatch>,
    ) -> Result<Option<Seen>, GitError> {
        let heads = parse_heads(refs);
        let Some(tip) = heads.get(default_branch).cloned() else {
            return Ok(None);
        };
        let known = match known {
            Some(known) if !self.has_every_tip(known).await? => None,
            known => known,
        };
        let read_before: Vec<&str> = known
            .into_iter()
            .flat_map(|known| std::iter::once(&known.tip).chain(&known.upstream))
            .map(String::as_str)
            .collect();
        let local_reached = match known {
            Some(known) if known.tip == tip => Reached::Since(Vec::new()),
            Some(known) if self.is_ancestor(&known.tip, &tip).await? => {
                Reached::Since(self.reached(&tip, &read_before).await?)
            }
            _ => Reached::Unknown,
        };
        let local = self
            .reading(
                Place {
                    name: default_branch.to_owned(),
                    tip: tip.clone(),
                },
                local_reached,
                &heads,
            )
            .await?;
        let upstream = match upstream_of(refs, default_branch) {
            Some(place) => {
                let continues = match known {
                    Some(GitWatch {
                        upstream: Some(old),
                        ..
                    }) => old == &place.tip || self.reaches(old, &place.tip).await?,
                    Some(_) => true,
                    None => false,
                };
                let reached = if continues {
                    let mut excluded = read_before.clone();
                    if matches!(local.reached, Reached::Since(_)) {
                        excluded.push(&tip);
                    }
                    Reached::Since(self.reached(&place.tip, &excluded).await?)
                } else {
                    Reached::Unknown
                };
                Some(self.reading(place, reached, &heads).await?)
            }
            None => None,
        };
        Ok(Some(Seen {
            repository: self.git_dir.clone(),
            default_branch: default_branch.to_owned(),
            heads: heads
                .into_iter()
                .map(|(branch, tip)| Head { branch, tip })
                .collect(),
            local,
            upstream,
        }))
    }

    async fn reading(
        &self,
        place: Place,
        reached: Reached,
        heads: &BTreeMap<String, String>,
    ) -> Result<Reading, GitError> {
        let outside = self.outside(&place.tip, heads).await?;
        Ok(Reading {
            place,
            outside,
            reached,
        })
    }

    pub async fn outside(
        &self,
        tip: &str,
        heads: &BTreeMap<String, String>,
    ) -> Result<BTreeSet<String>, GitError> {
        let merged = parse_heads(
            &self
                .text(&["for-each-ref", REFS, &format!("--merged={tip}"), HEADS])
                .await?,
        );
        Ok(heads
            .iter()
            .filter(|(branch, hash)| merged.get(*branch) != Some(*hash))
            .map(|(branch, _)| branch.clone())
            .collect())
    }

    async fn has_every_tip(&self, known: &GitWatch) -> Result<bool, GitError> {
        for tip in std::iter::once(&known.tip).chain(&known.upstream) {
            if !self.has_commit(tip).await? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub async fn has_commit(&self, commit: &str) -> Result<bool, GitError> {
        Ok(self.commit_of(commit).await?.is_some())
    }

    pub async fn commit_of(&self, name: &str) -> Result<Option<String>, GitError> {
        let peeled = format!("{name}^{{commit}}");
        let args = [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &peeled,
        ];
        let output = self.run(&args).await?;
        match output.status.code() {
            Some(0) => Ok(Some(
                String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            )),
            Some(NOT_A_COMMIT) => Ok(None),
            _ => Err(self.failed(&args, &output)),
        }
    }

    /// Which of `commits` this repository has; none when git cannot tell.
    pub async fn present(&self, commits: &[String]) -> BTreeSet<String> {
        let mut args = vec![
            "rev-list",
            "--no-walk",
            "--ignore-missing",
            "--end-of-options",
        ];
        args.extend(commits.iter().map(String::as_str));
        match self.text(&args).await {
            Ok(listed) => listed.lines().map(str::to_owned).collect(),
            Err(_) => BTreeSet::new(),
        }
    }

    pub async fn reachable_from(&self, commit: &str) -> Result<BTreeSet<String>, GitError> {
        let listed = self.text(&["rev-list", "--end-of-options", commit]).await?;
        Ok(listed.lines().map(str::to_owned).collect())
    }

    /// `git merge-base --is-ancestor` fails, rather than answering no, for a commit this repository no longer has.
    pub async fn reaches(&self, ancestor: &str, of: &str) -> Result<bool, GitError> {
        match self.is_ancestor(ancestor, of).await {
            Err(GitError::Failed { .. }) => Ok(false),
            answer => answer,
        }
    }

    pub async fn is_ancestor(&self, ancestor: &str, of: &str) -> Result<bool, GitError> {
        let args = ["merge-base", "--is-ancestor", ancestor, of];
        let output = self.run(&args).await?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(NOT_AN_ANCESTOR) => Ok(false),
            _ => Err(self.failed(&args, &output)),
        }
    }

    async fn reached(&self, tip: &str, read_before: &[&str]) -> Result<Vec<Commit>, GitError> {
        let mut args = vec![
            "log",
            "-z",
            "--format=%H%n%B",
            "--ignore-missing",
            tip,
            "--not",
        ];
        args.extend_from_slice(read_before);
        let output = self.bytes(&args).await?;
        Ok(String::from_utf8_lossy(&output)
            .split('\0')
            .filter_map(|record| {
                let (hash, message) = record.split_once('\n')?;
                Some(Commit {
                    hash: hash.to_owned(),
                    refs: ids::refs_values(message),
                })
            })
            .collect())
    }

    /// The first line git prints for `args`, or `None` when git fails or prints nothing. What
    /// git said on failure is dropped, so it reaches no error message and no log.
    pub(super) async fn line(&self, args: &[&str]) -> Option<String> {
        let output = self.run(args).await.ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let line = text.lines().next()?.trim();
        (!line.is_empty()).then(|| line.to_owned())
    }

    async fn text(&self, args: &[&str]) -> Result<String, GitError> {
        let output = self.bytes(args).await?;
        String::from_utf8(output).map_err(|_| GitError::NotText(command_name(args)))
    }

    async fn bytes(&self, args: &[&str]) -> Result<Vec<u8>, GitError> {
        let output = self.run(args).await?;
        if !output.status.success() {
            return Err(self.failed(args, &output));
        }
        Ok(output.stdout)
    }

    fn failed(&self, args: &[&str], output: &Output) -> GitError {
        GitError::Failed {
            command: command_name(args),
            repository: self.git_dir.clone(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        }
    }

    async fn run(&self, args: &[&str]) -> Result<Output, GitError> {
        let mut command = Command::new("git");
        environment::prepare(command.as_std_mut(), &self.configuration);
        let mut child = command
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .arg("--no-pager")
            .arg("--no-replace-objects")
            .arg(format!("--git-dir={}", self.git_dir))
            .args(["-c", "log.showSignature=false"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take())
        else {
            unreachable!("both streams are piped");
        };
        let finished = tokio::time::timeout(self.limit, async {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let (read_out, read_err, status) = tokio::join!(
                stdout.read_to_end(&mut out),
                stderr.read_to_end(&mut err),
                child.wait()
            );
            read_out?;
            read_err?;
            Ok::<_, std::io::Error>(Output {
                status: status?,
                stdout: out,
                stderr: err,
            })
        })
        .await;
        match finished {
            Ok(output) => Ok(output?),
            Err(_) => {
                let _ = child.kill().await;
                Err(GitError::TooSlow {
                    command: command_name(args),
                    repository: self.git_dir.clone(),
                    limit: self.limit,
                })
            }
        }
    }
}

pub fn landing_tips(refs: &str, default_branch: &str) -> LandingTips {
    LandingTips {
        local: parse_heads(refs).remove(default_branch),
        upstream: upstream_of(refs, default_branch).map(|place| place.tip),
    }
}

fn command_name(args: &[&str]) -> String {
    args.first().copied().unwrap_or_default().to_owned()
}

fn parse_refs(text: &str) -> impl Iterator<Item = (&str, &str, &str)> {
    text.lines().filter_map(|line| {
        let mut fields = line.split(' ');
        Some((fields.next()?, fields.next()?, fields.next().unwrap_or("")))
    })
}

fn parse_heads(text: &str) -> BTreeMap<String, String> {
    parse_refs(text)
        .filter_map(|(tip, name, _)| Some((name.strip_prefix(HEADS)?.to_owned(), tip.to_owned())))
        .collect()
}

fn upstream_of(text: &str, branch: &str) -> Option<Place> {
    let local = format!("{HEADS}{branch}");
    let (_, _, upstream) = parse_refs(text).find(|(_, name, _)| *name == local)?;
    let (tip, _, _) =
        parse_refs(text).find(|(_, name, _)| !upstream.is_empty() && *name == upstream)?;
    let name = upstream.strip_prefix(REMOTES)?;
    Some(Place {
        name: name.to_owned(),
        tip: tip.to_owned(),
    })
}
