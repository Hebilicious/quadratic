//! The Quadratique line of `AGENTS.md`: finding it, and bringing a file to it.
//!
//! An instruction line is a line, outside any fenced code block, that is exactly the instruction
//! from its first character, followed by nothing but spaces or tabs before its line ending.
//! Trailing spaces are accepted because Markdown renders the line the same; a quotation (`> `),
//! an indented line (an indented code block) and a line inside a fence are text about the
//! instruction, not the instruction, and are never touched. Every other byte stays as it is.

use crate::shipped;

pub enum Found {
    /// The current line, and no earlier one.
    Current,
    /// An earlier build's line, possibly beside the current one: `updated` brings the file to the
    /// current line alone.
    Earlier,
    /// No instruction line.
    Absent,
}

struct Line<'a> {
    body: &'a [u8],
    ending: &'a [u8],
}

fn lines(text: &[u8]) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (line, after) = match rest.iter().position(|byte| *byte == b'\n') {
            Some(end) => rest.split_at(end + 1),
            None => (rest, &[][..]),
        };
        let body_end = if line.ends_with(b"\r\n") {
            line.len() - 2
        } else if line.ends_with(b"\n") {
            line.len() - 1
        } else {
            line.len()
        };
        lines.push(Line {
            body: &line[..body_end],
            ending: &line[body_end..],
        });
        rest = after;
    }
    lines
}

/// The opening of a fence: its character and how many of them, after at most three spaces.
fn fence(body: &[u8]) -> Option<(u8, usize)> {
    let indent = body.iter().take_while(|byte| **byte == b' ').count();
    if indent > 3 {
        return None;
    }
    let rest = &body[indent..];
    let mark = *rest.first()?;
    if mark != b'`' && mark != b'~' {
        return None;
    }
    let length = rest.iter().take_while(|byte| **byte == mark).count();
    (length >= 3).then_some((mark, length))
}

fn closes(body: &[u8], (mark, length): (u8, usize)) -> bool {
    fence(body).is_some_and(|(found, found_length)| {
        let indent = body.iter().take_while(|byte| **byte == b' ').count();
        found == mark
            && found_length >= length
            && body[indent + found_length..]
                .iter()
                .all(|byte| *byte == b' ' || *byte == b'\t')
    })
}

fn is_instruction(body: &[u8], instruction: &str) -> bool {
    let end = body
        .iter()
        .rposition(|byte| *byte != b' ' && *byte != b'\t')
        .map_or(0, |last| last + 1);
    &body[..end] == instruction.as_bytes()
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Current,
    Earlier,
    Text,
}

fn kinds(lines: &[Line<'_>], current: &str) -> Vec<Kind> {
    let mut open: Option<(u8, usize)> = None;
    lines
        .iter()
        .map(|line| {
            if let Some(opening) = open {
                if closes(line.body, opening) {
                    open = None;
                }
                return Kind::Text;
            }
            if let Some(opening) = fence(line.body) {
                open = Some(opening);
                return Kind::Text;
            }
            if is_instruction(line.body, current) {
                Kind::Current
            } else if shipped::AGENTS_LINES
                .iter()
                .any(|earlier| is_instruction(line.body, earlier))
            {
                Kind::Earlier
            } else {
                Kind::Text
            }
        })
        .collect()
}

pub fn find(text: &[u8], current: &str) -> Found {
    let kinds = kinds(&lines(text), current);
    if kinds.contains(&Kind::Earlier) {
        Found::Earlier
    } else if kinds.contains(&Kind::Current) {
        Found::Current
    } else {
        Found::Absent
    }
}

/// `text` with the current line as its only instruction line: an earlier line becomes the
/// current line where it stands (the first one, unless the current line is already there), other
/// earlier lines are removed, and a file without any gets the line at its end, in its own line
/// ending.
pub fn updated(text: &[u8], current: &str) -> Vec<u8> {
    let lines = lines(text);
    let kinds = kinds(&lines, current);
    if !kinds.contains(&Kind::Earlier) && !kinds.contains(&Kind::Current) {
        return appended(text, current);
    }
    let mut keep_current = !kinds.contains(&Kind::Current);
    let mut result = Vec::with_capacity(text.len());
    for (line, kind) in lines.iter().zip(kinds) {
        match kind {
            Kind::Earlier if keep_current => {
                keep_current = false;
                result.extend_from_slice(current.as_bytes());
                result.extend_from_slice(line.ending);
            }
            Kind::Earlier => {}
            Kind::Current | Kind::Text => {
                result.extend_from_slice(line.body);
                result.extend_from_slice(line.ending);
            }
        }
    }
    result
}

fn appended(text: &[u8], current: &str) -> Vec<u8> {
    let ending: &[u8] = match lines(text).first() {
        Some(line) if line.ending == b"\r\n" => b"\r\n",
        _ => b"\n",
    };
    let blank_line_before = [ending, ending].concat();
    let separator: &[u8] = if text.is_empty() || text.ends_with(&blank_line_before) {
        b""
    } else if text.ends_with(ending) {
        ending
    } else {
        &blank_line_before
    };
    [text, separator, current.as_bytes(), ending].concat()
}

#[cfg(test)]
mod tests {
    use super::{Found, find, updated};

    const NEW: &str = "We use Quadratique for issues: every task is an issue there. Use the `quadratique` skill (`.agents/skills/quadratique/SKILL.md`) for anything about tasks.";
    const OLD: &str = "We use Quadratic for issues: every task is an issue there. Use the `quadratic` skill (`.agents/skills/quadratic/SKILL.md`) for anything about tasks.";

    fn update(text: &str) -> String {
        String::from_utf8(updated(text.as_bytes(), NEW)).unwrap()
    }

    #[test]
    fn a_standalone_earlier_line_becomes_the_current_line_where_it_stands() {
        assert_eq!(
            update(&format!("# Agents\n\n{OLD}\n\nRun the tests.")),
            format!("# Agents\n\n{NEW}\n\nRun the tests.")
        );
    }

    #[test]
    fn a_quotation_a_fenced_example_and_an_indented_line_are_left_as_they_are() {
        let around = |last: &str| {
            format!(
                "> {OLD}\n\n```md\n{OLD}\n```\n\n~~~~\n{OLD}\n~~~\n{OLD}\n~~~~\n\n    {OLD}\n\n{last}\n"
            )
        };
        let once = update(&around(OLD));
        assert_eq!(once, around(NEW));
        assert!(matches!(find(once.as_bytes(), NEW), Found::Current));
    }

    #[test]
    fn of_several_earlier_lines_the_first_becomes_current_and_the_others_go() {
        assert_eq!(
            update(&format!("{OLD}\nmiddle\n{OLD}\nend\n")),
            format!("{NEW}\nmiddle\nend\n")
        );
    }

    #[test]
    fn beside_the_current_line_an_earlier_line_goes() {
        let text = format!("{OLD}\n{NEW}\n");
        assert!(matches!(find(text.as_bytes(), NEW), Found::Earlier));
        assert_eq!(update(&text), format!("{NEW}\n"));
    }

    #[test]
    fn trailing_spaces_still_make_the_line_and_crlf_and_a_missing_final_newline_are_kept() {
        assert_eq!(
            update(&format!("# A\r\n{OLD}  \t\r\nend")),
            format!("# A\r\n{NEW}\r\nend")
        );
        assert_eq!(update(&format!("# A\r\n{OLD}")), format!("# A\r\n{NEW}"));
    }

    #[test]
    fn a_file_without_the_line_gets_it_at_its_end_in_its_own_line_ending() {
        assert_eq!(update("# A\r\nrun"), format!("# A\r\nrun\r\n\r\n{NEW}\r\n"));
        assert_eq!(update("# A\n"), format!("# A\n\n{NEW}\n"));
        assert_eq!(update(""), format!("{NEW}\n"));
        assert!(matches!(
            find(format!("> {NEW}\n").as_bytes(), NEW),
            Found::Absent
        ));
    }

    #[test]
    fn a_second_update_changes_nothing() {
        let text = format!("> {OLD}\n{OLD}\n{OLD}\n");
        let once = update(&text);
        assert_eq!(update(&once), once);
    }
}
