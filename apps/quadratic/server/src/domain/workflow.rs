use serde::{Deserialize, Serialize};

use super::failure::{Failure, MoveProblem};

pub type Status = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Backlog,
    Unstarted,
    Started,
    Completed,
    Canceled,
}

impl Category {
    pub fn is_resolved(self) -> bool {
        matches!(self, Category::Completed | Category::Canceled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Move {
    Claim,
    Review,
    Merge,
    Release,
}

impl Move {
    fn allows(self, category: Category) -> bool {
        match self {
            Move::Claim | Move::Review => category == Category::Started,
            Move::Merge => matches!(category, Category::Started | Category::Completed),
            Move::Release => category == Category::Unstarted,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStatus {
    pub key: Status,
    pub name: String,
    pub category: Category,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moves {
    pub claim: Status,
    pub review: Status,
    pub merge: Status,
    pub release: Status,
}

impl Moves {
    fn each(&self) -> [(Move, &Status); 4] {
        [
            (Move::Claim, &self.claim),
            (Move::Review, &self.review),
            (Move::Merge, &self.merge),
            (Move::Release, &self.release),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    pub statuses: Vec<WorkflowStatus>,
    pub moves: Moves,
}

impl Workflow {
    pub fn category_of(&self, status: &str) -> Option<Category> {
        self.statuses
            .iter()
            .find(|candidate| candidate.key == status)
            .map(|found| found.category)
    }

    pub(super) fn keys(&self) -> Vec<Status> {
        self.statuses
            .iter()
            .map(|status| status.key.clone())
            .collect()
    }

    pub fn position(&self, status: &str) -> Option<usize> {
        self.statuses
            .iter()
            .position(|candidate| candidate.key == status)
    }

    pub fn is_before(&self, status: &str, target: &str) -> bool {
        matches!(
            (self.position(status), self.position(target)),
            (Some(status), Some(target)) if status < target
        )
    }

    pub fn after_merge(&self, status: &str) -> Result<(), Failure> {
        if self.category_of(status).is_none() {
            return Err(Failure::UnknownStatus {
                status: status.to_string(),
                statuses: self.keys(),
            });
        }
        let after: Vec<Status> = self
            .statuses
            .iter()
            .skip_while(|candidate| candidate.key != self.moves.merge)
            .skip(1)
            .filter(|candidate| candidate.category != Category::Canceled)
            .map(|candidate| candidate.key.clone())
            .collect();
        if after.iter().any(|candidate| candidate == status) {
            Ok(())
        } else {
            Err(Failure::NotAfterMerge {
                status: status.to_string(),
                statuses: after,
            })
        }
    }

    pub fn first_of(&self, category: Category) -> Option<&Status> {
        self.statuses
            .iter()
            .find(|status| status.category == category)
            .map(|status| &status.key)
    }

    pub fn validate(&self) -> Result<(), Failure> {
        for (index, status) in self.statuses.iter().enumerate() {
            if self.statuses[..index]
                .iter()
                .any(|earlier| earlier.key == status.key)
            {
                return Err(Failure::DuplicateStatus {
                    status: status.key.clone(),
                });
            }
        }
        for (r#move, status) in self.moves.each() {
            let problem = match self.category_of(status) {
                None => Some(MoveProblem::Missing),
                Some(category) if !r#move.allows(category) => Some(MoveProblem::Category),
                Some(_)
                    if matches!(r#move, Move::Claim | Move::Review)
                        && *status == self.moves.merge =>
                {
                    Some(MoveProblem::Merge)
                }
                Some(_) => None,
            };
            if let Some(problem) = problem {
                return Err(Failure::InvalidMove {
                    r#move,
                    status: status.clone(),
                    problem,
                });
            }
        }
        Ok(())
    }
}
