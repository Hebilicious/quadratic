const MOST_DIGITS: usize = 9;

pub fn in_branch(branch: &str, project: &str) -> Vec<String> {
    let lower = branch.to_ascii_lowercase();
    let prefix = format!("{}-", project.to_ascii_lowercase());
    let bytes = lower.as_bytes();
    let mut ids = Vec::new();
    for (start, _) in lower.match_indices(&prefix) {
        let after_boundary = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let digits_at = start + prefix.len();
        let digits = lower[digits_at..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        let end = digits_at + digits;
        let before_boundary = bytes
            .get(end)
            .is_none_or(|next| !next.is_ascii_alphanumeric());
        if after_boundary
            && before_boundary
            && let Some(id) = issue_id(project, &lower[digits_at..end])
        {
            push_once(&mut ids, id);
        }
    }
    ids
}

pub fn refs_values(message: &str) -> Vec<String> {
    message
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("Refs")
                .then(|| value.trim().to_owned())
        })
        .filter(|value| !value.is_empty())
        .collect()
}

pub fn in_trailers(values: &[String], project: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for value in values {
        for token in value.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            let Some((key, number)) = token.rsplit_once('-') else {
                continue;
            };
            if key.eq_ignore_ascii_case(project)
                && let Some(id) = issue_id(project, number)
            {
                push_once(&mut ids, id);
            }
        }
    }
    ids
}

fn issue_id(project: &str, number: &str) -> Option<String> {
    let well_formed = (1..=MOST_DIGITS).contains(&number.len())
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && !number.starts_with('0');
    well_formed.then(|| format!("{project}-{number}"))
}

fn push_once(ids: &mut Vec<String>, id: String) {
    if !ids.contains(&id) {
        ids.push(id);
    }
}
