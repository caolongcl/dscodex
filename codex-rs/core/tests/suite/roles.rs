use anyhow::Result;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;

const ROLE_SPEC_OPEN_TAG: &str = "<role_spec>";

fn developer_texts(input: &[Value]) -> Vec<String> {
    input
        .iter()
        .filter(|item| item.get("role").and_then(Value::as_str) == Some("developer"))
        .filter_map(|item| item.get("content")?.as_array().cloned())
        .flatten()
        .filter_map(|content| Some(content.get("text")?.as_str()?.to_string()))
        .collect()
}

fn count_messages_containing(texts: &[String], target: &str) -> usize {
    texts.iter().filter(|text| text.contains(target)).count()
}

fn simple_text_turn(text: &str) -> Op {
    Op::UserInput {
        items: vec![UserInput::Text {
            text: text.into(),
            text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
        responsesapi_client_metadata: None,
        additional_context: Default::default(),
        thread_settings: Default::default(),
    }
}

fn text_turn_with_role(text: &str, role: &str) -> Op {
    Op::UserInput {
        items: vec![UserInput::Text {
            text: text.into(),
            text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
        responsesapi_client_metadata: None,
        additional_context: Default::default(),
        thread_settings: codex_protocol::protocol::ThreadSettingsOverrides {
            role: Some(role.to_string()),
            ..Default::default()
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_role_instructions_by_default() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let req = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let test = test_codex().build(&server).await?;
    test.codex.submit(simple_text_turn("hello")).await?;
    wait_for_event(&test.codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let dev_texts = developer_texts(&req.single_request().input());
    assert_eq!(count_messages_containing(&dev_texts, ROLE_SPEC_OPEN_TAG), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn builtin_role_injects_role_spec_fragment() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let req = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let test = test_codex()
        .with_config(|config| {
            config.role = Some("writer".to_string());
        })
        .build(&server)
        .await?;
    test.codex.submit(simple_text_turn("hello")).await?;
    wait_for_event(&test.codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let dev_texts = developer_texts(&req.single_request().input());
    assert_eq!(count_messages_containing(&dev_texts, ROLE_SPEC_OPEN_TAG), 1);
    assert_eq!(
        count_messages_containing(&dev_texts, "Role: Writer"),
        1,
        "expected the writer preset spec in developer messages, got {dev_texts:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_role_file_shadows_builtin_and_unknown_degrades() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let req = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let test = test_codex()
        .with_config(|config| {
            let roles_dir = config.codex_home.join("roles");
            std::fs::create_dir_all(&roles_dir).expect("create roles dir");
            std::fs::write(
                roles_dir.join("writer.md"),
                "# Tang dynasty poet\nYou compose regulated verse.",
            )
            .expect("write custom role");
            config.role = Some("writer".to_string());
        })
        .build(&server)
        .await?;
    test.codex.submit(simple_text_turn("hello")).await?;
    wait_for_event(&test.codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let dev_texts = developer_texts(&req.single_request().input());
    assert_eq!(
        count_messages_containing(&dev_texts, "Tang dynasty poet"),
        1,
        "custom role file should shadow the builtin preset, got {dev_texts:?}"
    );
    assert_eq!(count_messages_containing(&dev_texts, "Role: Writer"), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_role_name_degrades_to_default() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let req = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let test = test_codex()
        .with_config(|config| {
            config.role = Some("no-such-role".to_string());
        })
        .build(&server)
        .await?;
    test.codex.submit(simple_text_turn("hello")).await?;
    wait_for_event(&test.codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let dev_texts = developer_texts(&req.single_request().input());
    assert_eq!(count_messages_containing(&dev_texts, ROLE_SPEC_OPEN_TAG), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_settings_switch_role_mid_session() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let req = mount_sse_sequence(
        &server,
        vec![
            sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
            sse(vec![ev_response_created("resp-3"), ev_completed("resp-3")]),
        ],
    )
    .await;

    let test = test_codex().build(&server).await?;

    // Turn 1: no role.
    test.codex.submit(simple_text_turn("hello")).await?;
    wait_for_event(&test.codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    // Turn 2: switch to the researcher preset via thread settings.
    test.codex
        .submit(text_turn_with_role("hello again", "researcher"))
        .await?;
    wait_for_event(&test.codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    // Turn 3: switch back to the default coding agent.
    test.codex
        .submit(text_turn_with_role("one more", "default"))
        .await?;
    wait_for_event(&test.codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let requests = req.requests();
    assert_eq!(requests.len(), 3, "expected three model requests");

    let turn1_texts = developer_texts(&requests[0].input());
    assert_eq!(
        count_messages_containing(&turn1_texts, ROLE_SPEC_OPEN_TAG),
        0
    );

    let turn2_texts = developer_texts(&requests[1].input());
    assert_eq!(
        count_messages_containing(&turn2_texts, "Role: Researcher"),
        1,
        "expected researcher spec after switching roles, got {turn2_texts:?}"
    );

    // After clearing, the latest developer context must not re-assert a role.
    let turn3_texts = developer_texts(&requests[2].input());
    let role_mentions_after_clear = turn3_texts
        .iter()
        .filter(|text| text.contains("Role: Researcher"))
        .count();
    assert_eq!(
        role_mentions_after_clear, 1,
        "history keeps the turn-2 fragment but no new role fragment should be added, got {turn3_texts:?}"
    );
    Ok(())
}
