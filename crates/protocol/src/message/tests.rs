use super::*;

fn sample_session_id() -> SessionId {
    SessionId::from_bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef])
}

fn sample_request() -> Request {
    Request {
        session_id: sample_session_id(),
        generation: PromptGeneration::new(42),
        cwd: "/home/user/project".to_owned(),
        cols: 120,
        last_exit_code: 0,
        duration_ms: Some(1500),
        keymap: "main".to_owned(),
        env_vars: vec![],
    }
}

fn sample_render_result() -> RenderResult {
    RenderResult {
        session_id: sample_session_id(),
        generation: PromptGeneration::new(42),
        left1: "~/project  main".to_owned(),
        left2: "❯ ".to_owned(),
        meta: String::new(),
    }
}

fn sample_update() -> Update {
    Update {
        session_id: sample_session_id(),
        generation: PromptGeneration::new(42),
        left1: "~/project  main *2".to_owned(),
        left2: "❯ ".to_owned(),
        meta: String::new(),
    }
}

fn sample_hello() -> Hello {
    Hello {
        build_id: Some(BuildId::new("12345:1700000000000000000".to_owned())),
    }
}

fn sample_hello_ack() -> HelloAck {
    HelloAck {
        build_id: Some(BuildId::new("12345:1700000000000000000".to_owned())),
        env_var_names: vec![],
    }
}

// -- SessionId --

#[test]
fn test_session_id_hex_round_trip() -> Result<(), ProtocolError> {
    let sid = sample_session_id();
    let hex = sid.to_string();
    assert_eq!(hex, "0123456789abcdef");

    let parsed = SessionId::from_hex(hex.as_bytes())?;
    assert_eq!(parsed, sid);
    Ok(())
}

#[test]
fn test_session_id_uppercase_hex() -> Result<(), ProtocolError> {
    let parsed = SessionId::from_hex(b"0123456789ABCDEF")?;
    assert_eq!(parsed, sample_session_id());
    Ok(())
}

#[test]
fn test_session_id_invalid_length() {
    let result = SessionId::from_hex(b"0123");
    assert!(matches!(result, Err(ProtocolError::InvalidField { .. })));
}

#[test]
fn test_session_id_invalid_hex() {
    let result = SessionId::from_hex(b"012345678XABCDEF");
    assert!(matches!(result, Err(ProtocolError::InvalidField { .. })));
}

// -- Request round-trip --

#[test]
fn test_request_round_trip() -> Result<(), ProtocolError> {
    let req = sample_request();
    let wire = req.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::Request(req));
    Ok(())
}

#[test]
fn test_request_serializer_uses_protocol_version() -> Result<(), ProtocolError> {
    let req = sample_request();
    let wire = req.to_wire();
    let (field, _rest) = netstring::decode(&wire)?;

    assert_eq!(field, PROTOCOL_VERSION.to_string().as_bytes());
    Ok(())
}

#[test]
fn test_request_with_none_duration() -> Result<(), ProtocolError> {
    let mut req = sample_request();
    req.duration_ms = None;
    let wire = req.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::Request(req));
    Ok(())
}

#[test]
fn test_request_with_negative_exit_code() -> Result<(), ProtocolError> {
    let mut req = sample_request();
    req.last_exit_code = -1;
    let wire = req.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::Request(req));
    Ok(())
}

#[test]
fn test_request_with_utf8_cwd() -> Result<(), ProtocolError> {
    let mut req = sample_request();
    req.cwd = "/home/ユーザー/プロジェクト".to_owned();
    let wire = req.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::Request(req));
    Ok(())
}

fn assert_env_vars_round_trip(env_vars: Vec<(String, String)>) -> Result<(), ProtocolError> {
    let mut req = sample_request();
    req.env_vars = env_vars;
    let wire = req.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::Request(req));
    Ok(())
}

#[test]
fn test_request_env_vars_round_trip_cases() -> Result<(), ProtocolError> {
    for env_vars in [
        vec![],
        vec![("PATH".to_owned(), "/usr/local/bin:/usr/bin".to_owned())],
        vec![
            ("PATH".to_owned(), "/usr/local/bin:/usr/bin".to_owned()),
            ("HOME".to_owned(), "/home/user".to_owned()),
        ],
    ] {
        assert_env_vars_round_trip(env_vars)?;
    }
    Ok(())
}

// -- RenderResult round-trip --

#[test]
fn test_render_result_round_trip() -> Result<(), ProtocolError> {
    let rr = sample_render_result();
    let wire = rr.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::RenderResult(rr));
    Ok(())
}

#[test]
fn test_render_result_empty_prompts() -> Result<(), ProtocolError> {
    let rr = RenderResult {
        session_id: sample_session_id(),
        generation: PromptGeneration::new(0),
        left1: String::new(),
        left2: String::new(),
        meta: String::new(),
    };
    let wire = rr.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::RenderResult(rr));
    Ok(())
}

#[test]
fn test_render_result_with_meta_round_trip() -> Result<(), ProtocolError> {
    let rr = RenderResult {
        meta: "viins\x1e\x1b[32m❯\x1b[0m\x1fvicmd\x1e\x1b[32m❮\x1b[0m".to_owned(),
        ..sample_render_result()
    };
    let wire = rr.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::RenderResult(rr));
    Ok(())
}

// -- Update round-trip --

#[test]
fn test_update_round_trip() -> Result<(), ProtocolError> {
    let upd = sample_update();
    let wire = upd.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::Update(upd));
    Ok(())
}

#[test]
fn test_update_with_meta_round_trip() -> Result<(), ProtocolError> {
    let upd = Update {
        meta: "viins\x1e\x1b[32m❯\x1b[0m\x1fvicmd\x1e\x1b[32m❮\x1b[0m".to_owned(),
        ..sample_update()
    };
    let wire = upd.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::Update(upd));
    Ok(())
}

// -- Hello round-trip --

#[test]
fn test_hello_round_trip() -> Result<(), ProtocolError> {
    let hello = sample_hello();
    let wire = hello.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::Hello(hello));
    Ok(())
}

#[test]
fn test_hello_none_build_id() -> Result<(), ProtocolError> {
    let hello = Hello { build_id: None };
    let wire = hello.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::Hello(hello));
    Ok(())
}

// -- HelloAck round-trip --

#[test]
fn test_hello_ack_round_trip() -> Result<(), ProtocolError> {
    let ack = sample_hello_ack();
    let wire = ack.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::HelloAck(ack));
    Ok(())
}

#[test]
fn test_hello_ack_with_env_var_names_round_trip() -> Result<(), ProtocolError> {
    let ack = HelloAck {
        build_id: Some(BuildId::new("test:123".to_owned())),
        env_var_names: vec!["AWS_PROFILE".to_owned(), "TERRAFORM_WORKSPACE".to_owned()],
    };
    let wire = ack.to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::HelloAck(ack));
    Ok(())
}

// -- Error cases --

#[test]
fn test_from_wire_empty_input() {
    let result = Message::from_wire(b"");
    assert!(matches!(
        result,
        Err(ProtocolError::WrongFieldCount {
            expected: 2,
            got: 0
        })
    ));
}

#[test]
fn test_from_wire_unknown_type() {
    // Build: version=1, type=X
    let mut wire = netstring::encode(b"1");
    wire.extend_from_slice(&netstring::encode(b"X"));
    let result = Message::from_wire(&wire);
    assert!(matches!(result, Err(ProtocolError::UnknownMessageType)));
}

#[test]
fn test_from_wire_wrong_field_count() {
    // Build a Q message with only 5 fields instead of 10
    let mut wire = netstring::encode(b"1");
    wire.extend_from_slice(&netstring::encode(b"Q"));
    wire.extend_from_slice(&netstring::encode(b"0123456789abcdef"));
    wire.extend_from_slice(&netstring::encode(b"1"));
    wire.extend_from_slice(&netstring::encode(b"/tmp"));
    let result = Message::from_wire(&wire);
    assert!(matches!(
        result,
        Err(ProtocolError::WrongFieldCount { expected: 10, .. })
    ));
}

#[test]
fn test_from_wire_invalid_generation() {
    // Hand-build a Q frame with a non-numeric generation field.
    let mut wire = netstring::encode(b"1");
    wire.extend_from_slice(&netstring::encode(b"Q"));
    wire.extend_from_slice(&netstring::encode(b"0123456789abcdef"));
    wire.extend_from_slice(&netstring::encode(b"not_a_number"));
    wire.extend_from_slice(&netstring::encode(b"/tmp"));
    wire.extend_from_slice(&netstring::encode(b"80"));
    wire.extend_from_slice(&netstring::encode(b"0"));
    wire.extend_from_slice(&netstring::encode(b""));
    wire.extend_from_slice(&netstring::encode(b"main"));
    wire.extend_from_slice(&netstring::encode(b""));
    let result = Message::from_wire(&wire);
    assert!(matches!(result, Err(ProtocolError::InvalidField { .. })));
}

// -- Env var edge cases --

#[test]
fn test_request_env_vars_edge_cases() -> Result<(), ProtocolError> {
    let cases = [
        vec![(String::new(), "value".to_owned())],
        vec![("PATH".to_owned(), String::new())],
        vec![(String::new(), String::new())],
        vec![("PATH".to_owned(), "/usr/bin:dir=with=equals".to_owned())],
        vec![("PATH".to_owned(), "/usr/local/bin:".repeat(500))],
        vec![(
            "PATH".to_owned(),
            "/usr/bin;rm -rf /:$(evil):`evil`:$((1+1))".to_owned(),
        )],
    ];

    for env_vars in cases {
        assert_env_vars_round_trip(env_vars)?;
    }
    Ok(())
}

// -- Env var wire-level edge cases (hand-crafted bytes) --

/// Build a 10-field Q message from raw bytes with a custom meta field,
/// then decode it and return the `env_vars`.
fn env_vars_from_wire(meta: &[u8]) -> Result<Vec<(String, String)>, ProtocolError> {
    let mut wire = Vec::new();
    netstring::encode_into(&mut wire, b"1");
    netstring::encode_into(&mut wire, b"Q");
    netstring::encode_into(&mut wire, b"0123456789abcdef");
    netstring::encode_into(&mut wire, b"1");
    netstring::encode_into(&mut wire, b"/tmp");
    netstring::encode_into(&mut wire, b"80");
    netstring::encode_into(&mut wire, b"0");
    netstring::encode_into(&mut wire, b"");
    netstring::encode_into(&mut wire, b"main");
    netstring::encode_into(&mut wire, meta);
    let Message::Request(req) = Message::from_wire(&wire)? else {
        unreachable!("expected Message::Request from wire with meta: {meta:?}");
    };
    Ok(req.env_vars)
}

/// Null byte in the wire meta field splits into separate entries.
/// Rust `String` cannot contain null bytes, so the encoder never produces
/// this — the decoder handles it consistently by treating null as separator.
fn assert_env_vars_from_wire(meta: &[u8], expected: &[(&str, &str)]) -> Result<(), ProtocolError> {
    let vars = env_vars_from_wire(meta)?;
    let expected = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(vars, expected);
    Ok(())
}

#[test]
fn test_request_env_vars_wire_cases() -> Result<(), ProtocolError> {
    type ExpectedEnvVar<'a> = &'a [(&'a str, &'a str)];
    type WireEnvVarCase<'a> = (&'a [u8], ExpectedEnvVar<'a>);

    let cases: [WireEnvVarCase<'_>; 5] = [
        (
            b"PATH=/usr/bin\0INJECT=evil",
            &[("PATH", "/usr/bin"), ("INJECT", "evil")],
        ),
        (b"PATH=\xff\xfe/usr/bin", &[]),
        (b"", &[]),
        (b"=", &[("", "")]),
        (b"MALFORMED_NO_EQUALS", &[]),
    ];

    for (meta, expected) in cases {
        assert_env_vars_from_wire(meta, expected)?;
    }
    Ok(())
}

#[test]
fn test_status_request_round_trip() -> Result<(), ProtocolError> {
    let req = StatusRequest;
    let wire = Message::StatusRequest(req).to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::StatusRequest(StatusRequest));
    Ok(())
}

#[test]
fn test_status_request_rejects_trailing_fields() {
    let mut wire = Message::StatusRequest(StatusRequest).to_wire();
    netstring::encode_into(&mut wire, b"extra");

    let parsed = Message::from_wire(&wire);
    assert!(matches!(
        parsed,
        Err(ProtocolError::WrongFieldCount {
            expected: 2,
            got: 3
        })
    ));
}

#[test]
fn test_status_response_round_trip() -> Result<(), ProtocolError> {
    let resp = StatusResponse {
        pid: 12345,
        uptime_secs: 3600,
        cache_hits: 100,
        cache_misses: 10,
        cache_evictions: 2,
        cache_entries: 42,
        inflight_coalesces: 5,
        requests_total: 110,
        stale_discards: 3,
        slow_computes_started: 10,
        slow_compute_duration_us: 500_000,
        git_timeouts: 1,
        custom_module_timeouts: 0,
        active_sessions: 3,
        sessions_pruned: 7,
        connections_total: 50,
        connections_active: 2,
        config_generation: ConfigGeneration::new(1),
        config_reloads: 1,
        config_reload_errors: 0,
    };
    let wire = Message::StatusResponse(resp.clone()).to_wire();
    let parsed = Message::from_wire(&wire)?;
    assert_eq!(parsed, Message::StatusResponse(resp));
    Ok(())
}
