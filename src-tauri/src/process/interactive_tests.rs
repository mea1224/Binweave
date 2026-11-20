use super::*;
use std::{
    io::{BufRead, Write},
    time::Duration,
};

#[test]
fn validates_answers_and_limits_permissions_to_one_request() {
    for agent in [AgentId::Codex, AgentId::Claude] {
        let raw = if agent == AgentId::Codex {
            json!({"id":9,"method":"item/commandExecution/requestApproval","params":{"command":"echo test"}})
        } else {
            json!({"type":"control_request","request_id":"9","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo test"}}})
        };
        let r = request(agent, &raw).unwrap();
        assert!(r.approval);
        assert!(response(agent, &raw, &r, &json!({"decision":"always"})).is_err());
        let allow = response(agent, &raw, &r, &json!({"decision":"allow"})).unwrap();
        let deny = response(agent, &raw, &r, &json!({"decision":"deny"})).unwrap();
        if agent == AgentId::Codex {
            assert_eq!(allow["result"]["decision"], "accept");
            assert_eq!(deny["result"]["decision"], "decline");
        } else {
            assert_eq!(
                allow["response"]["response"]["updatedInput"],
                raw["request"]["input"]
            );
            assert_eq!(deny["response"]["response"]["behavior"], "deny");
        }
    }
    let raw = json!({"id":3,"method":"item/tool/requestUserInput","params":{"questions":[{"id":"q","question":"选择方案","options":[{"label":"A"},{"label":"B"}]}]}});
    let r = request(AgentId::Codex, &raw).unwrap();
    assert!(response(AgentId::Codex, &raw, &r, &json!({"answers":{"q":[]}})).is_err());
    let answer = response(
        AgentId::Codex,
        &raw,
        &r,
        &json!({"answers":{"q":["手工方案"]}}),
    )
    .unwrap();
    assert_eq!(answer["result"]["answers"]["q"]["answers"][0], "手工方案");
    assert!(request(AgentId::Codex, &json!({"method":"unknown/request"})).is_err());
}

#[test]
fn remembered_commands_are_exact_scoped_and_cannot_grant_other_permissions() {
    let cwd = std::env::temp_dir();
    for agent in [AgentId::Codex, AgentId::Claude] {
        let raw = if agent == AgentId::Codex {
            json!({"id":1,"method":"item/commandExecution/requestApproval","params":{"command":"echo test","cwd":"/work","turnId":"one"}})
        } else {
            json!({"type":"control_request","request_id":"1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"one","input":{"command":"echo test"}}})
        };
        let r = request(agent, &raw).unwrap();
        assert!(r.can_remember);
        let allow = response(
            agent,
            &raw,
            &r,
            &json!({"decision":"allow","remember":true}),
        )
        .unwrap();
        assert_eq!(
            allow,
            response(agent, &raw, &r, &json!({"decision":"allow"})).unwrap()
        );
        assert!(response(agent, &raw, &r, &json!({"decision":"deny","remember":true})).is_err());
        assert!(response(
            agent,
            &raw,
            &r,
            &json!({"decision":"allow","remember":"yes"})
        )
        .is_err());
        let key = permission_key(agent, &raw, "run", &cwd).unwrap();
        assert_ne!(
            Some(key.clone()),
            permission_key(agent, &raw, "other", &cwd)
        );
        assert_ne!(
            Some(key.clone()),
            permission_key(agent, &raw, "run", &cwd.join("other"))
        );
        let mut next = raw.clone();
        if agent == AgentId::Codex {
            next["params"]["turnId"] = json!("two");
            next["id"] = json!(2);
        } else {
            next["request"]["tool_use_id"] = json!("two");
            next["request_id"] = json!("2");
        }
        assert_eq!(Some(key.clone()), permission_key(agent, &next, "run", &cwd));
        if agent == AgentId::Codex {
            next["params"]["command"] = json!("echo other");
        } else {
            next["request"]["input"]["command"] = json!("echo other");
        }
        assert_ne!(Some(key.clone()), permission_key(agent, &next, "run", &cwd));
        let mut privileged = raw.clone();
        if agent == AgentId::Codex {
            privileged["params"]["additionalPermissions"] = json!({"network":true});
        } else {
            privileged["request"]["input"]["dangerouslyDisableSandbox"] = json!(true);
        }
        assert_ne!(Some(key), permission_key(agent, &privileged, "run", &cwd));
    }
    let raw = json!({"id":1,"method":"item/permissions/requestApproval","params":{"permissions":{"network":true}}});
    let r = request(AgentId::Codex, &raw).unwrap();
    assert!(!r.can_remember);
    assert!(response(
        AgentId::Codex,
        &raw,
        &r,
        &json!({"decision":"allow","remember":true})
    )
    .is_err());
    assert!(Broker::default().remembered.lock().unwrap().is_empty());
}

#[test]
#[ignore = "Duplex CLI fixture invoked by interactive tests"]
fn duplex_fixture() {
    let codex = std::env::var("DUPLEX_AGENT").unwrap() == "codex";
    let read = || {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let emit = |v: Value| {
        println!("{v}");
        std::io::stdout().flush().unwrap();
    };
    let init = read();
    let mode = std::env::var("DUPLEX_MODE").unwrap_or_default();
    if mode == "elicitation" {
        let action = std::env::var("MCP_ACTION").unwrap();
        let kind = std::env::var("MCP_MODE").unwrap();
        let mut params = json!({"mode":"form","serverName":"fixture","threadId":"session","message":"确认测试","requestedSchema":{"type":"object","properties":{}}});
        if kind == "form" {
            params["requestedSchema"] = json!({"type":"object","properties":{"count":{"type":"integer","minimum":1},"agree":{"type":"boolean"}},"required":["count","agree"]});
        }
        if kind == "url" {
            params = json!({"mode":"url","serverName":"fixture","threadId":"session","message":"手动处理","url":"https://example.test/login","elicitationId":"auth"});
        }
        // These IDs used to be mistaken for replies to our initialization RPCs.
        let id = if kind == "url" {
            json!("thread")
        } else if kind == "form" {
            json!("init")
        } else {
            json!(0)
        };
        emit(json!({"id":id,"method":"mcpServer/elicitation/request","params":params}));
        let reply = read();
        assert_eq!(reply["id"], id);
        assert_eq!(reply["result"]["action"], action);
        assert!(reply["result"].get("decision").is_none());
        if action == "accept" && kind == "form" {
            assert_eq!(reply["result"]["content"], json!({"count":2,"agree":false}));
        }
        if action != "accept" || kind == "url" {
            assert!(reply["result"].get("content").is_none());
        }
        emit(
            json!({"method":"item/completed","params":{"item":{"type":"agentMessage","text":"原进程已收到 MCP 回答"}}}),
        );
        emit(json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}));
        return;
    }
    if mode == "thinking" {
        for _ in 0..400 {
            emit(if codex {
                json!({"method":"item/reasoning/textDelta","params":{"delta":"仍在思考"}})
            } else {
                json!({"type":"stream_event","event":{"delta":{"type":"thinking_delta","thinking":"仍在思考"}}})
            });
            std::thread::sleep(Duration::from_millis(10));
        }
        return;
    }
    if mode == "repeat_denial" {
        for id in 1..=3 {
            emit(if codex {
                json!({"id":id,"method":"item/commandExecution/requestApproval","params":{"command":"python arbitrary.py"}})
            } else {
                json!({"type":"control_request","request_id":id.to_string(),"request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"python arbitrary.py"}}})
            });
            read();
        }
        std::thread::sleep(Duration::from_secs(10));
        return;
    }
    if mode == "approval" {
        let scoped = std::env::var("DUPLEX_SCOPED").as_deref() == Ok("1");
        emit(if codex {
            json!({"id":19,"method":"item/commandExecution/requestApproval","params":{"command":if scoped {"cat sample.bin"} else {"echo test"},"cwd":std::env::current_dir().unwrap()}})
        } else {
            if scoped {
                json!({"type":"control_request","request_id":"19","request":{"subtype":"can_use_tool","tool_name":"Read","input":{"file_path":"sample.bin"}}})
            } else {
                json!({"type":"control_request","request_id":"19","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo test"}}})
            }
        });
        let reply = read();
        let allow = std::env::var("DUPLEX_DECISION").as_deref() == Ok("allow");
        if codex {
            assert_eq!(reply["id"], 19);
            assert_eq!(
                reply["result"]["decision"],
                if allow { "accept" } else { "decline" }
            );
            emit(json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}));
        } else {
            assert_eq!(reply["response"]["request_id"], "19");
            assert_eq!(
                reply["response"]["response"]["behavior"],
                if allow { "allow" } else { "deny" }
            );
            emit(json!({"type":"result","is_error":false,"result":"授权选择已收到"}));
        }
        return;
    }
    if mode == "flood" {
        for _ in 0..100_000 {
            emit(json!({"type":"fixture","text":"持续输出"}));
        }
        std::thread::sleep(Duration::from_secs(20));
        return;
    }
    if mode == "exit_wait" {
        emit(json!({"type":"result","is_error":false,"result":"done"}));
        std::thread::sleep(Duration::from_secs(20));
        return;
    }
    if codex {
        assert_eq!(init["method"], "initialize");
        emit(json!({"id":"init","result":{"catalog":"x".repeat(2 * 1024 * 1024)}}));
        assert_eq!(read()["method"], "initialized");
        let thread = read();
        if std::env::var("DUPLEX_RESUME").as_deref() == Ok("1") {
            assert_eq!(thread["method"], "thread/resume");
            assert_eq!(
                thread["params"]["threadId"],
                "27d22593-8519-444a-b1cb-6575a8e63514"
            );
        } else {
            assert_eq!(thread["method"], "thread/start");
        }
        emit(
            json!({"id":"thread","result":{"thread":{"id":"27d22593-8519-444a-b1cb-6575a8e63514"}}}),
        );
        assert_eq!(read()["method"], "turn/start");
        emit(
            json!({"method":"item/agentMessage/delta","params":{"itemId":"a","delta":"正在处理"}}),
        );
        emit(
            json!({"id":19,"method":"item/tool/requestUserInput","params":{"questions":[{"id":"q","question":"输入方案","options":[{"label":"A"}]}]}}),
        );
        let reply = read();
        assert_eq!(reply["id"], 19);
        assert_eq!(reply["result"]["answers"]["q"]["answers"][0], "手动选项");
        emit(
            json!({"method":"item/completed","params":{"item":{"type":"agentMessage","text":"已收到回答"}}}),
        );
        emit(json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}));
    } else {
        assert_eq!(init["request"]["subtype"], "initialize");
        emit(
            json!({"type":"control_response","response":{"request_id":"init","subtype":"success"}}),
        );
        assert_eq!(read()["type"], "user");
        emit(
            json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"正在处理"}}}),
        );
        emit(
            json!({"type":"control_request","request_id":"19","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"q","multiSelect":true,"options":[{"label":"A"}]}]}}}),
        );
        let reply = read();
        assert_eq!(reply["response"]["request_id"], "19");
        assert_eq!(
            reply["response"]["response"]["updatedInput"]["answers"]["q"],
            "手动选项"
        );
        emit(json!({"type":"result","is_error":false,"result":"已收到回答"}));
    }
}

#[tokio::test]
async fn analyst_denies_command_elevation_without_operator_or_remembered_grants() {
    for agent in [AgentId::Codex, AgentId::Claude] {
        for repeated in [false, true] {
            let broker = Arc::new(Broker::default());
            let spec = RunSpec {
                interactive: Some(Config {
                    elicitation_policy: Default::default(),
                    analyst: true,
                    broker: Arc::clone(&broker),
                    run_id: "r".into(),
                    prompt_id: "t".into(),
                    session: None,
                }),
                agent,
                executable: std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                args: vec![
                    "--exact".into(),
                    "process::interactive::tests::duplex_fixture".into(),
                    "--ignored".into(),
                    "--nocapture".into(),
                ],
                cwd: std::env::temp_dir(),
                env: HashMap::from([
                    (
                        "DUPLEX_AGENT".into(),
                        if agent == AgentId::Codex {
                            "codex"
                        } else {
                            "claude"
                        }
                        .into(),
                    ),
                    (
                        "DUPLEX_MODE".into(),
                        if repeated {
                            "repeat_denial"
                        } else {
                            "approval"
                        }
                        .into(),
                    ),
                    ("DUPLEX_DECISION".into(), "deny".into()),
                ]),
                input: Some("分析".into()),
                deadline: Duration::from_secs(5),
            };
            let (_stop, cancel) = oneshot::channel();
            let rejected = std::sync::atomic::AtomicBool::new(false);
            assert_eq!(
                super::run(
                    &spec,
                    spec.interactive.as_ref().unwrap(),
                    cancel,
                    |output| {
                        if let Some(request) = output.interaction {
                            assert_eq!(request.status, "blocked");
                            assert!(request.title.starts_with("分析方权限边界已拒绝"));
                            rejected.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        Ok(())
                    }
                )
                .await,
                if repeated {
                    Err(RunError::Failed(format!("READ_POLICY_BLOCKED：{:?} 连续请求超出分析方权限边界，已停止重复尝试。请人工补证或调整合规读取方式后单侧重试。", agent)))
                } else {
                    Ok(0)
                }
            );
            assert!(rejected.load(std::sync::atomic::Ordering::Relaxed));
            assert!(broker.pending.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn duplex_replies_reach_the_original_process_and_reject_wrong_or_expired_turns() {
    for (agent, resume) in [
        (AgentId::Codex, false),
        (AgentId::Codex, true),
        (AgentId::Claude, false),
    ] {
        let broker = Arc::new(Broker::default());
        let spec = RunSpec {
            interactive: Some(Config {
                elicitation_policy: Default::default(),
                analyst: false,
                broker: Arc::clone(&broker),
                run_id: "run".into(),
                prompt_id: "turn".into(),
                session: resume.then(|| "27d22593-8519-444a-b1cb-6575a8e63514".into()),
            }),
            agent,
            executable: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            args: vec![
                "--exact".into(),
                "process::interactive::tests::duplex_fixture".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            cwd: std::env::temp_dir(),
            env: HashMap::from([
                (
                    "DUPLEX_RESUME".into(),
                    if resume { "1" } else { "0" }.into(),
                ),
                (
                    "DUPLEX_AGENT".into(),
                    if agent == AgentId::Codex {
                        "codex"
                    } else {
                        "claude"
                    }
                    .into(),
                ),
            ]),
            input: Some("任务".into()),
            deadline: Duration::from_secs(5),
        };
        let (_stop, cancel) = oneshot::channel();
        let (requests, mut receive) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let id: String = receive.recv().await.unwrap();
            let answer = json!({"answers":{"q":["手动选项"]}});
            assert!(broker
                .respond("wrong", "turn", &id, answer.clone())
                .await
                .is_err());
            broker
                .respond("run", "turn", &id, answer.clone())
                .await
                .unwrap();
            assert!(broker.respond("run", "turn", &id, answer).await.is_err());
        });
        let completed = Mutex::new(false);
        let streamed = Mutex::new(false);
        let result = super::run(
            &spec,
            spec.interactive.as_ref().unwrap(),
            cancel,
            |output| {
                if output.summary.as_deref() == Some("正在处理") {
                    *streamed.lock().unwrap() = true;
                }
                if let Some(r) = output.interaction {
                    assert!(
                        *streamed.lock().unwrap(),
                        "文字必须在进程等待回答及最终完成之前送达"
                    );
                    requests.send(r.id).unwrap();
                }
                if output.summary.as_deref() == Some("已收到回答") {
                    *completed.lock().unwrap() = true;
                }
                Ok(())
            },
        )
        .await;
        assert_eq!(result, Ok(0));
        task.await.unwrap();
        assert!(*completed.lock().unwrap());
        assert!(*streamed.lock().unwrap());
        assert!(spec
            .interactive
            .as_ref()
            .unwrap()
            .broker
            .pending
            .lock()
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn approval_choices_are_sent_back_to_both_agents() {
    for analyst in [false, true] {
        for (agent, decision) in [
            (AgentId::Codex, "allow"),
            (AgentId::Codex, "deny"),
            (AgentId::Claude, "allow"),
            (AgentId::Claude, "deny"),
            (AgentId::Codex, "remember"),
            (AgentId::Claude, "remember"),
        ] {
            if analyst && decision == "remember" {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("sample.bin"), b"evidence").unwrap();
            let broker = Arc::new(Broker::default());
            let config = Config {
                elicitation_policy: Default::default(),
                analyst,
                broker: Arc::clone(&broker),
                run_id: "run".into(),
                prompt_id: "turn".into(),
                session: None,
            };
            let spec = RunSpec {
                interactive: Some(Config {
                    elicitation_policy: Default::default(),
                    analyst: false,
                    broker: Arc::clone(&broker),
                    run_id: "run".into(),
                    prompt_id: "turn".into(),
                    session: None,
                }),
                agent,
                executable: std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                args: vec![
                    "--exact".into(),
                    "process::interactive::tests::duplex_fixture".into(),
                    "--ignored".into(),
                    "--nocapture".into(),
                ],
                cwd: dir.path().to_owned(),
                env: HashMap::from([
                    (
                        "DUPLEX_AGENT".into(),
                        if agent == AgentId::Codex {
                            "codex"
                        } else {
                            "claude"
                        }
                        .into(),
                    ),
                    ("DUPLEX_MODE".into(), "approval".into()),
                    (
                        "DUPLEX_SCOPED".into(),
                        if analyst { "1" } else { "0" }.into(),
                    ),
                    (
                        "DUPLEX_DECISION".into(),
                        if decision == "remember" {
                            "allow"
                        } else {
                            decision
                        }
                        .into(),
                    ),
                ]),
                input: Some("任务".into()),
                deadline: Duration::from_secs(5),
            };
            let (_stop, cancel) = oneshot::channel();
            let (requests, mut receive) = mpsc::unbounded_channel();
            let task = tokio::spawn(async move {
                let id: String = receive.recv().await.unwrap();
                // Waiting for a human must outlive the thinking watchdog.
                tokio::time::sleep(Duration::from_millis(1750)).await;
                broker
                .respond("run", "turn", &id, json!({"decision": if decision == "remember" { "allow" } else { decision }, "remember": decision == "remember"}))
                .await
                .unwrap();
            });
            let result = super::run_with_timeout(
                &spec,
                &config,
                cancel,
                |output| {
                    if let Some(request) = output.interaction {
                        if analyst {
                            assert!(!request.can_remember);
                            assert_eq!(request.status, "pending");
                        }
                        requests.send(request.id).unwrap();
                    }
                    Ok(())
                },
                Duration::from_millis(1500),
            )
            .await;
            assert_eq!(result, Ok(0), "{agent:?} {decision}");
            task.await.unwrap();
            assert_eq!(
                config.broker.remembered.lock().unwrap().len(),
                usize::from(decision == "remember")
            );
            if decision == "remember" {
                let config = Config {
                    elicitation_policy: Default::default(),
                    analyst: false,
                    broker: Arc::clone(&config.broker),
                    run_id: "run".into(),
                    prompt_id: "next-turn".into(),
                    session: None,
                };
                let (_stop, cancel) = oneshot::channel();
                let automated = Mutex::new(0);
                let result = super::run(&spec, &config, cancel, |output| {
                    if let Some(request) = output.interaction {
                        assert_eq!(request.status, "answered");
                        assert!(request.title.contains("自动允许"));
                        *automated.lock().unwrap() += 1;
                    }
                    Ok(())
                })
                .await;
                assert_eq!(result, Ok(0));
                assert_eq!(*automated.lock().unwrap(), 1);
                assert!(config.broker.pending.lock().unwrap().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn continuous_thinking_hits_the_five_minute_watchdog_without_waiting_for_turn_deadline() {
    let broker = Arc::new(Broker::default());
    let config = Config {
        elicitation_policy: Default::default(),
        analyst: false,
        broker: Arc::clone(&broker),
        run_id: "run".into(),
        prompt_id: "turn".into(),
        session: None,
    };
    let spec = RunSpec {
        interactive: Some(Config {
            elicitation_policy: Default::default(),
            analyst: false,
            broker: Arc::clone(&broker),
            run_id: "run".into(),
            prompt_id: "turn".into(),
            session: None,
        }),
        agent: AgentId::Claude,
        executable: std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        args: vec![
            "--exact".into(),
            "process::interactive::tests::duplex_fixture".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
        cwd: std::env::temp_dir(),
        env: HashMap::from([
            ("DUPLEX_AGENT".into(), "claude".into()),
            ("DUPLEX_MODE".into(), "thinking".into()),
        ]),
        input: Some("任务".into()),
        deadline: Duration::from_secs(5),
    };
    let (_stop, cancel) = oneshot::channel();
    assert_eq!(
        super::run_with_timeout(
            &spec,
            &config,
            cancel,
            |_| Ok(()),
            Duration::from_millis(80)
        )
        .await,
        Err(RunError::ThinkingTimedOut)
    );
    assert!(broker.pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stopping_pending_request_closes_reply_routes() {
    let broker = Arc::new(Broker::default());
    let spec = RunSpec {
        interactive: Some(Config {
            elicitation_policy: Default::default(),
            analyst: false,
            broker: Arc::clone(&broker),
            run_id: "r".into(),
            prompt_id: "t".into(),
            session: None,
        }),
        agent: AgentId::Codex,
        executable: std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        args: vec![
            "--exact".into(),
            "process::interactive::tests::duplex_fixture".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
        cwd: std::env::temp_dir(),
        env: HashMap::from([("DUPLEX_AGENT".into(), "codex".into())]),
        input: Some("任务".into()),
        deadline: Duration::from_secs(5),
    };
    let (stop, cancel) = oneshot::channel();
    let stop = Mutex::new(Some(stop));
    let result = super::run(&spec, spec.interactive.as_ref().unwrap(), cancel, |o| {
        if o.interaction.is_some() {
            let _ = stop.lock().unwrap().take().unwrap().send(());
        }
        Ok(())
    })
    .await;
    assert_eq!(result, Err(RunError::Cancelled));
    assert!(broker.pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pending_request_times_out_and_removes_routes() {
    let broker = Arc::new(Broker::default());
    let config = Config {
        elicitation_policy: Default::default(),
        analyst: false,
        broker: Arc::clone(&broker),
        run_id: "r".into(),
        prompt_id: "t".into(),
        session: None,
    };
    let spec = RunSpec {
        interactive: None,
        agent: AgentId::Codex,
        executable: std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        args: vec![
            "--exact".into(),
            "process::interactive::tests::duplex_fixture".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
        cwd: std::env::temp_dir(),
        env: HashMap::from([("DUPLEX_AGENT".into(), "codex".into())]),
        input: Some("任务".into()),
        deadline: Duration::from_millis(250),
    };
    let (_stop, cancel) = oneshot::channel();
    assert_eq!(
        super::run(&spec, &config, cancel, |_| Ok(())).await,
        Err(RunError::TimedOut)
    );
    assert!(broker.pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_preempts_buffered_output_and_exit_wait() {
    for mode in ["flood", "exit_wait"] {
        let config = Config {
            elicitation_policy: Default::default(),
            analyst: false,
            broker: Arc::new(Broker::default()),
            run_id: "r".into(),
            prompt_id: "t".into(),
            session: None,
        };
        let spec = RunSpec {
            interactive: None,
            agent: AgentId::Claude,
            executable: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            args: vec![
                "--exact".into(),
                "process::interactive::tests::duplex_fixture".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            cwd: std::env::temp_dir(),
            env: HashMap::from([
                ("DUPLEX_AGENT".into(), "claude".into()),
                ("DUPLEX_MODE".into(), mode.into()),
            ]),
            input: Some("test".into()),
            deadline: Duration::from_secs(10),
        };
        let (stop, cancel) = oneshot::channel();
        let stop = Mutex::new(Some(stop));
        let signalled = Arc::new(Mutex::new(None));
        let result = super::run(&spec, &config, cancel, |output| {
            if output.raw.contains(if mode == "flood" {
                "持续输出"
            } else {
                "done"
            }) {
                if let Some(stop) = stop.lock().unwrap().take() {
                    let signalled = Arc::clone(&signalled);
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        *signalled.lock().unwrap() = Some(std::time::Instant::now());
                        let _ = stop.send(());
                    });
                }
            }
            Ok(())
        })
        .await;
        assert_eq!(result, Err(RunError::Cancelled), "{mode}");
        assert!(
            signalled.lock().unwrap().unwrap().elapsed() < Duration::from_secs(1),
            "{mode}"
        );
        assert!(config.broker.pending.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn mcp_decisions_preserve_process_ids_and_pause_both_timeouts() {
    for kind in ["form", "confirmation", "url"] {
        for action in ["accept", "decline", "cancel"] {
            let broker = Arc::new(Broker::default());
            let config = Config {
                elicitation_policy: Default::default(),
                analyst: true,
                broker: broker.clone(),
                run_id: "run".into(),
                prompt_id: "turn".into(),
                session: None,
            };
            let spec = RunSpec {
                interactive: None,
                agent: AgentId::Codex,
                executable: std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                args: vec![
                    "--exact".into(),
                    "process::interactive::tests::duplex_fixture".into(),
                    "--ignored".into(),
                    "--nocapture".into(),
                ],
                cwd: std::env::temp_dir(),
                env: HashMap::from([
                    ("DUPLEX_AGENT".into(), "codex".into()),
                    ("DUPLEX_MODE".into(), "elicitation".into()),
                    ("MCP_MODE".into(), kind.into()),
                    ("MCP_ACTION".into(), action.into()),
                ]),
                input: Some("任务".into()),
                deadline: Duration::from_millis(600),
            };
            let (_stop, cancel) = oneshot::channel();
            let (tx, mut rx) = mpsc::unbounded_channel::<String>();
            let task = tokio::spawn(async move {
                let id = rx.recv().await.unwrap();
                tokio::time::sleep(Duration::from_millis(700)).await;
                let content = if kind == "form" {
                    json!({"count":2,"agree":false})
                } else {
                    json!({})
                };
                let answer = json!({"action":action,"content":content});
                assert!(broker
                    .respond("other", "turn", &id, answer.clone())
                    .await
                    .is_err());
                assert!(broker
                    .respond("run", "stale", &id, answer.clone())
                    .await
                    .is_err());
                if kind == "form" && action == "accept" {
                    assert!(broker
                        .respond(
                            "run",
                            "turn",
                            &id,
                            json!({"action":"accept","content":{"count":"2","agree":false}})
                        )
                        .await
                        .is_err());
                }
                broker
                    .respond("run", "turn", &id, answer.clone())
                    .await
                    .unwrap();
                assert!(broker.respond("run", "turn", &id, answer).await.is_err());
            });
            let resolved = Mutex::new(false);
            let result = run_with_timeout(
                &spec,
                &config,
                cancel,
                |o| {
                    if let Some(r) = o.interaction {
                        assert!(!r.approval);
                        if r.status == "pending" {
                            tx.send(r.id).unwrap();
                        } else {
                            assert_eq!(r.decision.as_deref(), Some(action));
                            assert_eq!(r.delivery.as_deref(), Some("delivered"));
                            *resolved.lock().unwrap() = true;
                        }
                    }
                    Ok(())
                },
                Duration::from_millis(600),
            )
            .await;
            assert_eq!(result, Ok(0), "{kind} {action}");
            task.await.unwrap();
            assert!(*resolved.lock().unwrap());
            assert!(config.broker.pending.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn failed_response_write_is_not_acknowledged_as_delivery() {
    let (mut input, output) = tokio::io::duplex(1024);
    drop(output);
    assert!(write(
        &mut input,
        &json!({"id":"init","result":{"action":"cancel"}})
    )
    .await
    .is_err());
}
