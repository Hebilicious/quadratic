//! The git check of the `stacked-on` branch rule. It runs only for a decision that needs it:
//! whether the caller can start an issue stacked on a base in review, in the repository that
//! the base's project is bound to and that the caller works in.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{LazyLock, Mutex, PoisonError};

use super::board::Board;
use super::{Context, Refused};
use crate::domain::{Caller, IssueId, STACKED_ON};
use crate::git::repository::{GIT_LIMIT, Repository};
use crate::store::AsOf;

static CHECKS: LazyLock<Mutex<BTreeMap<String, usize>>> = LazyLock::new(Mutex::default);

/// How many git processes the branch rule started in `repository`.
#[doc(hidden)]
pub fn branch_checks_in(repository: &str) -> usize {
    CHECKS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(repository)
        .copied()
        .unwrap_or(0)
}

fn counted(repository: &str) {
    *CHECKS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(repository.to_owned())
        .or_default() += 1;
}

pub(super) enum Scope {
    Issue(IssueId),
    Every,
}

/// The caller, with the base branches that git verified its branch contains. A caller with no
/// repository or branch, a board with no `stacked-on` link, or no base in review in the
/// caller's repository, starts no git process.
pub(super) async fn verified(context: &Context, scope: Scope) -> Result<Caller, Refused> {
    let mut caller = context.caller.clone();
    let copy = &caller.working_copy;
    let (Some(repository), Some(branch)) = (copy.repository.clone(), copy.branch.clone()) else {
        return Ok(caller);
    };
    let asking = caller.clone();
    let bases = context
        .read(AsOf::Now, move |view| {
            if !view.has_relation_kind(STACKED_ON)? {
                return Ok(BTreeSet::new());
            }
            let board = Board::load(view)?;
            let issues = match &scope {
                Scope::Issue(id) => Some(std::slice::from_ref(id)),
                Scope::Every => None,
            };
            Ok(board.branches_to_check(issues, &asking))
        })
        .await?;
    if bases.is_empty() {
        return Ok(caller);
    }
    let git = Repository::at(&repository, GIT_LIMIT);
    let tip = format!("refs/heads/{branch}");
    let mut contained = BTreeSet::new();
    for base in bases {
        counted(&repository);
        if git
            .reaches(&format!("refs/heads/{base}"), &tip)
            .await
            .unwrap_or(false)
        {
            contained.insert(base);
        }
    }
    caller.working_copy.contains = Some(contained);
    Ok(caller)
}
