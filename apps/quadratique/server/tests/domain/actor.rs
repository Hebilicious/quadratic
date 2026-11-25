use std::collections::BTreeMap;

use quadratique_server::domain::{Actor, ActorError, ActorKind, Binding};

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn detect(pairs: &[(&str, &str)], binding: Binding, worktree: Option<&str>) -> Actor {
    Actor::detect(&env(pairs), binding, worktree, Some("sam")).expect("a valid environment")
}

const FIX_LOGIN: Option<&str> = Some("/home/sam/code/demo/.worktrees/fix-login");

#[test]
fn a_security_agent_that_t3_drives_in_claude_code() {
    let actor = detect(
        &[
            ("QUADRATIQUE_ROLE", "security"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "session-1"),
            ("T3CODE_THREAD_ID", "thread-9"),
        ],
        Binding::Cli,
        FIX_LOGIN,
    );
    assert_eq!(
        actor,
        Actor {
            kind: ActorKind::Agent,
            name: "[security]claude-code(t3)@fix-login".into(),
            role: Some("security".into()),
            harness: Some("claude-code".into()),
            host: Some("t3".into()),
            host_thread: Some("thread-9".into()),
            parent: None,
            model: None,
            session: Some("session-1".into()),
        }
    );
}

#[test]
fn claude_code_run_directly() {
    let actor = detect(&[("CLAUDECODE", "1")], Binding::Cli, FIX_LOGIN);
    assert_eq!(actor.kind, ActorKind::Agent);
    assert_eq!(actor.name, "claude-code@fix-login");
}

#[test]
fn a_web_design_agent_in_codex() {
    let actor = detect(
        &[
            ("QUADRATIQUE_ROLE", "web-design"),
            ("CODEX_THREAD_ID", "019a-thread"),
        ],
        Binding::Mcp,
        Some("/home/sam/code/demo/.worktrees/add-search/"),
    );
    assert_eq!(actor.name, "[web-design]codex@add-search");
    assert_eq!(actor.harness.as_deref(), Some("codex"));
    assert_eq!(actor.session.as_deref(), Some("019a-thread"));
}

#[test]
fn an_agent_in_the_browser_is_browser() {
    let actor = detect(&[], Binding::WebMcp, None);
    assert_eq!(actor.kind, ActorKind::Agent);
    assert_eq!(actor.name, "browser");
}

#[test]
fn the_server_and_a_call_without_an_actor_header_are_fixed_agents() {
    let noisy = [("QUADRATIQUE_ROLE", "Not A Role"), ("CLAUDECODE", "1")];
    let server = detect(&noisy, Binding::Server, FIX_LOGIN);
    assert_eq!(
        (server.kind, server.name.as_str(), server.harness.as_deref()),
        (ActorKind::Agent, "quadratique", Some("quadratique"))
    );
    let anonymous = detect(&noisy, Binding::NoActorHeader, FIX_LOGIN);
    assert_eq!(anonymous, Actor::without_header());
    assert_eq!(
        (anonymous.kind, anonymous.name.as_str(), anonymous.harness),
        (ActorKind::Agent, "agent", None)
    );
}

#[test]
fn with_no_agent_variable_the_caller_is_the_human_by_login_name() {
    for falsy in ["", "0", "false", " No ", "OFF"] {
        let actor = detect(&[("CLAUDECODE", falsy)], Binding::Cli, FIX_LOGIN);
        assert_eq!(actor.kind, ActorKind::Human, "CLAUDECODE={falsy:?}");
        assert_eq!(actor.name, "sam");
    }
    let nameless = Actor::detect(&env(&[]), Binding::Cli, FIX_LOGIN, None).expect("valid");
    assert_eq!(nameless.name, "human");
}

#[test]
fn mcp_is_always_an_agent_and_an_absent_harness_reads_agent() {
    assert_eq!(detect(&[], Binding::Mcp, FIX_LOGIN).name, "agent@fix-login");
    assert_eq!(
        detect(&[("QUADRATIQUE_ROLE", "security")], Binding::Cli, None).name,
        "[security]agent"
    );
}

#[test]
fn ai_agent_gives_the_harness_before_the_agent_variables() {
    let actor = detect(
        &[("AI_AGENT", "Cursor_1.7"), ("CLAUDECODE", "1")],
        Binding::Cli,
        FIX_LOGIN,
    );
    assert_eq!(actor.harness.as_deref(), Some("cursor"));
    assert_eq!(actor.name, "cursor@fix-login");
}

#[test]
fn an_agent_variable_with_no_known_harness_makes_an_agent_without_one() {
    let actor = detect(&[("GEMINI_CLI", "1")], Binding::Cli, FIX_LOGIN);
    assert_eq!(actor.kind, ActorKind::Agent);
    assert_eq!(actor.harness, None);
    assert_eq!(actor.name, "agent@fix-login");
}

#[test]
fn the_override_gives_the_members_a_process_cannot_detect() {
    let fields = r#"{"harness":"Claude-Code","host":"conductor","hostThread":"c-42","parent":"[lead]codex@demo","model":"claude-opus-5-5","session":"s-7","name":"chosen","role":"admin"}"#;
    let actor = detect(
        &[
            ("QUADRATIQUE_ACTOR_FIELDS", fields),
            ("CODEX_THREAD_ID", "codex-thread"),
            ("T3CODE_THREAD_ID", "t3-thread"),
        ],
        Binding::Cli,
        FIX_LOGIN,
    );
    assert_eq!(
        actor,
        Actor {
            kind: ActorKind::Agent,
            name: "claude-code(conductor)@fix-login".into(),
            role: None,
            harness: Some("claude-code".into()),
            host: Some("conductor".into()),
            host_thread: Some("c-42".into()),
            parent: Some("[lead]codex@demo".into()),
            model: Some("claude-opus-5-5".into()),
            session: Some("s-7".into()),
        }
    );
}

#[test]
fn a_host_from_the_override_takes_no_thread_from_the_host_variable() {
    let actor = detect(
        &[
            ("QUADRATIQUE_ACTOR_FIELDS", r#"{"host":"conductor"}"#),
            ("T3CODE_THREAD_ID", "t3-thread"),
            ("CLAUDECODE", "1"),
        ],
        Binding::Cli,
        FIX_LOGIN,
    );
    assert_eq!(actor.host.as_deref(), Some("conductor"));
    assert_eq!(actor.host_thread, None);
}

#[test]
fn a_harness_that_fails_its_pattern_falls_to_the_next_source() {
    let actor = detect(
        &[
            ("QUADRATIQUE_ACTOR_FIELDS", r#"{"harness":"claude code"}"#),
            ("CLAUDECODE", "1"),
        ],
        Binding::Cli,
        FIX_LOGIN,
    );
    assert_eq!(actor.harness.as_deref(), Some("claude-code"));
}

#[test]
fn an_override_that_is_not_an_object_of_strings_gives_nothing() {
    for fields in ["not json", r#"["codex"]"#, r#"{"harness":7}"#] {
        let actor = detect(
            &[("QUADRATIQUE_ACTOR_FIELDS", fields), ("CLAUDECODE", "1")],
            Binding::Cli,
            FIX_LOGIN,
        );
        assert_eq!(actor.name, "claude-code@fix-login", "{fields}");
    }
}

#[test]
fn an_invalid_role_is_an_error_and_never_cut() {
    let too_long = "a".repeat(33);
    for role in [
        "Security",
        "web_design",
        "-lead",
        "lead-",
        too_long.as_str(),
    ] {
        assert_eq!(
            Actor::detect(
                &env(&[("QUADRATIQUE_ROLE", role)]),
                Binding::Mcp,
                FIX_LOGIN,
                None
            ),
            Err(ActorError::InvalidRole { role: role.into() }),
            "{role}"
        );
    }
    let longest = "a".repeat(32);
    assert_eq!(
        detect(&[("QUADRATIQUE_ROLE", &longest)], Binding::Cli, None).role,
        Some(longest)
    );
}

#[test]
fn values_longer_than_their_bound_are_cut() {
    let directory = "w".repeat(300);
    let worktree = format!("/home/sam/{directory}");
    let fields = format!(
        r#"{{"harness":"{}","model":"{}","hostThread":"{}"}}"#,
        "h".repeat(70),
        "m".repeat(200),
        "t".repeat(250)
    );
    let actor = detect(
        &[("QUADRATIQUE_ACTOR_FIELDS", &fields)],
        Binding::Mcp,
        Some(&worktree),
    );
    assert_eq!(actor.harness, Some("h".repeat(64)));
    assert_eq!(actor.model, Some("m".repeat(128)));
    assert_eq!(actor.host_thread, Some("t".repeat(200)));
    assert_eq!(
        actor.name,
        format!("{}@{}", "h".repeat(64), "w".repeat(255))
    );
}

#[test]
fn the_longest_name_fits_its_bound() {
    let fields = format!(
        r#"{{"harness":"{}","host":"{}"}}"#,
        "h".repeat(90),
        "o".repeat(90)
    );
    let worktree = format!("/{}", "w".repeat(400));
    let actor = detect(
        &[
            ("QUADRATIQUE_ROLE", &"r".repeat(32)),
            ("QUADRATIQUE_ACTOR_FIELDS", &fields),
        ],
        Binding::Cli,
        Some(&worktree),
    );
    assert_eq!(actor.name.chars().count(), 420);
}

#[test]
fn a_blank_session_is_no_session_and_a_valid_one_is_kept_exactly() {
    let from_variable = detect(
        &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", " \t")],
        Binding::Cli,
        FIX_LOGIN,
    );
    let from_override = detect(
        &[
            ("CLAUDECODE", "1"),
            ("QUADRATIQUE_ACTOR_FIELDS", r#"{"session":"  "}"#),
        ],
        Binding::Cli,
        FIX_LOGIN,
    );
    let padded = detect(
        &[
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", " session-1"),
        ],
        Binding::Cli,
        FIX_LOGIN,
    );

    assert_eq!(from_variable.session, None);
    assert_eq!(from_override.session, None);
    assert_eq!(padded.session.as_deref(), Some(" session-1"));
    assert_eq!(padded.agent_session(), Some(("claude-code", " session-1")));
}

#[test]
fn the_session_of_an_agent_needs_a_harness_and_a_session_that_are_not_blank() {
    let agent = |harness: Option<&str>, session: Option<&str>| Actor {
        harness: harness.map(Into::into),
        session: session.map(Into::into),
        ..detect(&[("CLAUDECODE", "1")], Binding::Cli, FIX_LOGIN)
    };

    assert_eq!(
        agent(Some("claude-code"), Some("session-1")).agent_session(),
        Some(("claude-code", "session-1"))
    );
    assert_eq!(agent(Some("claude-code"), Some(" ")).agent_session(), None);
    assert_eq!(agent(Some(" "), Some("session-1")).agent_session(), None);
    assert_eq!(agent(None, Some("session-1")).agent_session(), None);
}

#[test]
fn a_human_has_no_agent_session_whatever_fields_it_declares() {
    let human = detect(
        &[(
            "QUADRATIQUE_ACTOR_FIELDS",
            r#"{"harness":"claude-code","session":"session-1"}"#,
        )],
        Binding::Cli,
        FIX_LOGIN,
    );

    assert_eq!(human.kind, ActorKind::Human);
    assert_eq!(human.agent_session(), None);
}
