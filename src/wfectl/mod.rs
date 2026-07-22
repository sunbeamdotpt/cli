//! Remote workflow management via wfe-server gRPC API.
//!
//! This module embeds the `wfectl` command set into the sunbeam CLI so that
//! users can run `sunbeam workflow <subcommand>` instead of a separate binary.
//! Auth is handled by sunbeam's SSO flow (`sunbeam auth login`).
//!
//! The gRPC client and token resolution live in the SDK (`sdk::wfectl`);
//! they are re-exported here so the command modules can use `super::client`.

pub mod cancel;
/// Client (re-exported from the SDK).
pub use sdk::wfectl::client;
/// Definitions.
pub mod definitions;
/// Get.
pub mod get;
/// List.
pub mod list;
/// Logs.
pub mod logs;
/// Output.
pub mod output;
/// Publish.
pub mod publish;
/// Register.
pub mod register;
/// Resume.
pub mod resume;
/// Run.
pub mod run;
/// Search logs.
pub mod search_logs;
/// Struct util.
pub mod struct_util;
/// Suspend.
pub mod suspend;
/// Validate.
pub mod validate;
/// Watch.
pub mod watch;

#[cfg(test)]
mod tests {
    //! End-to-end tests for the wfectl command bodies against a stub
    //! wfe-server (tonic, in-process, plaintext loopback).

    use wfe_server_protos::tonic;
    use wfe_server_protos::tonic::transport::Server;
    use wfe_server_protos::wfe::v1::wfe_server::{Wfe, WfeServer};
    use wfe_server_protos::wfe::v1::*;

    use super::client::{self, AuthClient};
    use super::output::OutputFormat;

    // ── Stub server ─────────────────────────────────────────────────────

    #[derive(Default)]
    struct StubWfe;

    fn search_result() -> WorkflowSearchResult {
        WorkflowSearchResult {
            id: "wf-1".into(),
            definition_id: "ci".into(),
            version: 3,
            status: WorkflowStatus::Runnable as i32,
            reference: "refs/heads/main".into(),
            description: "CI pipeline".into(),
            create_time: None,
            name: "ci-42".into(),
        }
    }

    fn instance() -> WorkflowInstance {
        WorkflowInstance {
            id: "wf-1".into(),
            definition_id: "ci".into(),
            version: 3,
            description: "CI pipeline".into(),
            reference: "refs/heads/main".into(),
            status: WorkflowStatus::Runnable as i32,
            data: None,
            create_time: None,
            complete_time: None,
            execution_pointers: vec![ExecutionPointer {
                id: "ptr-1".into(),
                step_id: 1,
                step_name: "checkout".into(),
                status: PointerStatus::Complete as i32,
                start_time: None,
                end_time: None,
                retry_count: 0,
                active: false,
            }],
            name: "ci-42".into(),
        }
    }

    #[tonic::async_trait]
    impl Wfe for StubWfe {
        async fn register_workflow(
            &self,
            _request: tonic::Request<RegisterWorkflowRequest>,
        ) -> std::result::Result<tonic::Response<RegisterWorkflowResponse>, tonic::Status> {
            Ok(tonic::Response::new(RegisterWorkflowResponse {
                definitions: vec![],
            }))
        }

        async fn list_definitions(
            &self,
            _request: tonic::Request<ListDefinitionsRequest>,
        ) -> std::result::Result<tonic::Response<ListDefinitionsResponse>, tonic::Status> {
            Ok(tonic::Response::new(ListDefinitionsResponse {
                definitions: vec![DefinitionSummary {
                    name: "ci".into(),
                    id: "ci".into(),
                    version: 3,
                    description: "CI pipeline".into(),
                    step_count: 5,
                }],
            }))
        }

        async fn start_workflow(
            &self,
            _request: tonic::Request<StartWorkflowRequest>,
        ) -> std::result::Result<tonic::Response<StartWorkflowResponse>, tonic::Status> {
            Ok(tonic::Response::new(StartWorkflowResponse {
                workflow_id: "wf-new".into(),
                name: "ci-43".into(),
            }))
        }

        async fn get_workflow(
            &self,
            _request: tonic::Request<GetWorkflowRequest>,
        ) -> std::result::Result<tonic::Response<GetWorkflowResponse>, tonic::Status> {
            Ok(tonic::Response::new(GetWorkflowResponse {
                instance: Some(instance()),
            }))
        }

        async fn cancel_workflow(
            &self,
            _request: tonic::Request<CancelWorkflowRequest>,
        ) -> std::result::Result<tonic::Response<CancelWorkflowResponse>, tonic::Status> {
            Ok(tonic::Response::new(CancelWorkflowResponse {}))
        }

        async fn suspend_workflow(
            &self,
            _request: tonic::Request<SuspendWorkflowRequest>,
        ) -> std::result::Result<tonic::Response<SuspendWorkflowResponse>, tonic::Status> {
            Ok(tonic::Response::new(SuspendWorkflowResponse {}))
        }

        async fn resume_workflow(
            &self,
            _request: tonic::Request<ResumeWorkflowRequest>,
        ) -> std::result::Result<tonic::Response<ResumeWorkflowResponse>, tonic::Status> {
            Ok(tonic::Response::new(ResumeWorkflowResponse {}))
        }

        async fn search_workflows(
            &self,
            _request: tonic::Request<SearchWorkflowsRequest>,
        ) -> std::result::Result<tonic::Response<SearchWorkflowsResponse>, tonic::Status> {
            Ok(tonic::Response::new(SearchWorkflowsResponse {
                results: vec![search_result()],
                total: 1,
            }))
        }

        async fn publish_event(
            &self,
            _request: tonic::Request<PublishEventRequest>,
        ) -> std::result::Result<tonic::Response<PublishEventResponse>, tonic::Status> {
            Ok(tonic::Response::new(PublishEventResponse {
                event_id: "evt-1".into(),
            }))
        }

        type WatchLifecycleStream = tonic::codegen::tokio_stream::wrappers::ReceiverStream<
            std::result::Result<LifecycleEvent, tonic::Status>,
        >;

        async fn watch_lifecycle(
            &self,
            _request: tonic::Request<WatchLifecycleRequest>,
        ) -> std::result::Result<tonic::Response<Self::WatchLifecycleStream>, tonic::Status>
        {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tx.send(Ok(LifecycleEvent {
                event_time: None,
                workflow_id: "wf-1".into(),
                definition_id: "ci".into(),
                version: 3,
                event_type: 0,
                step_id: 1,
                step_name: "checkout".into(),
                error_message: String::new(),
            }))
            .await
            .ok();
            drop(tx);
            Ok(tonic::Response::new(
                tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx),
            ))
        }

        type StreamLogsStream = tonic::codegen::tokio_stream::wrappers::ReceiverStream<
            std::result::Result<LogEntry, tonic::Status>,
        >;

        async fn stream_logs(
            &self,
            _request: tonic::Request<StreamLogsRequest>,
        ) -> std::result::Result<tonic::Response<Self::StreamLogsStream>, tonic::Status> {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tx.send(Ok(LogEntry {
                workflow_id: "wf-1".into(),
                step_name: "checkout".into(),
                step_id: 1,
                stream: LogStream::Stdout as i32,
                data: b"hello from step\n".to_vec(),
                timestamp: None,
            }))
            .await
            .ok();
            drop(tx);
            Ok(tonic::Response::new(
                tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx),
            ))
        }

        async fn search_logs(
            &self,
            _request: tonic::Request<SearchLogsRequest>,
        ) -> std::result::Result<tonic::Response<SearchLogsResponse>, tonic::Status> {
            Ok(tonic::Response::new(SearchLogsResponse {
                results: vec![LogSearchResult {
                    workflow_id: "wf-1".into(),
                    definition_id: "ci".into(),
                    step_name: "checkout".into(),
                    line: "matched line".into(),
                    stream: LogStream::Stdout as i32,
                    timestamp: None,
                }],
                total: 1,
            }))
        }
    }

    // ── Harness ─────────────────────────────────────────────────────────

    fn logger() -> sdk::logger::Logger {
        sdk::logger::Logger::new(sdk::logger::TracingSink)
    }

    /// Start the stub server on a random loopback port and return a client.
    async fn stub_client() -> AuthClient {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let incoming = tonic::codegen::tokio_stream::wrappers::TcpListenerStream::new(listener);
        tokio::spawn(async move {
            Server::builder()
                .add_service(WfeServer::new(StubWfe))
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });
        client::build(&logger(), &format!("http://{addr}"), "test-token")
            .await
            .unwrap()
    }

    // ── Command tests ───────────────────────────────────────────────────

    #[tokio::test]
    async fn list_table_and_json() {
        let client = stub_client().await;
        let args = super::list::ListArgs {
            query: None,
            status: None,
            limit: 50,
            skip: 0,
        };
        super::list::run(&logger(), args, client, OutputFormat::Table)
            .await
            .unwrap();

        let client = stub_client().await;
        let args = super::list::ListArgs {
            query: Some("ci".into()),
            status: Some(super::list::StatusFilter::Runnable),
            limit: 10,
            skip: 0,
        };
        super::list::run(&logger(), args, client, OutputFormat::Json)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn get_table_and_json() {
        let client = stub_client().await;
        let args = super::get::GetArgs {
            workflow_id: "ci-42".into(),
        };
        super::get::run(&logger(), args, client, OutputFormat::Table)
            .await
            .unwrap();

        let client = stub_client().await;
        let args = super::get::GetArgs {
            workflow_id: "wf-1".into(),
        };
        super::get::run(&logger(), args, client, OutputFormat::Json)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn definitions_list_table_and_json() {
        let client = stub_client().await;
        let args = super::definitions::DefinitionsArgs {
            cmd: super::definitions::DefinitionsCmd::List,
        };
        super::definitions::run(args, client, OutputFormat::Table)
            .await
            .unwrap();

        let client = stub_client().await;
        let args = super::definitions::DefinitionsArgs {
            cmd: super::definitions::DefinitionsCmd::List,
        };
        super::definitions::run(args, client, OutputFormat::Json)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancel_suspend_resume() {
        let client = stub_client().await;
        super::cancel::run(
            &logger(),
            super::cancel::CancelArgs {
                workflow_id: "ci-42".into(),
            },
            client,
        )
        .await
        .unwrap();

        let client = stub_client().await;
        super::suspend::run(
            &logger(),
            super::suspend::SuspendArgs {
                workflow_id: "ci-42".into(),
            },
            client,
        )
        .await
        .unwrap();

        let client = stub_client().await;
        super::resume::run(
            &logger(),
            super::resume::ResumeArgs {
                workflow_id: "ci-42".into(),
            },
            client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn run_starts_workflow_with_inline_data() {
        let client = stub_client().await;
        let args = super::run::RunArgs {
            definition_id: "ci".into(),
            version: 3,
            data: None,
            data_json: Some("{\"branch\": \"main\"}".into()),
            name: None,
        };
        super::run::run(&logger(), args, client, OutputFormat::Table)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn run_rejects_invalid_data_json() {
        let client = stub_client().await;
        let args = super::run::RunArgs {
            definition_id: "ci".into(),
            version: 3,
            data: None,
            data_json: Some("{not json".into()),
            name: None,
        };
        let err = super::run::run(&logger(), args, client, OutputFormat::Table)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("valid JSON"), "err: {err}");
    }

    #[tokio::test]
    async fn publish_event_inline_data() {
        let client = stub_client().await;
        let args = super::publish::PublishArgs {
            event_name: "deploy.done".into(),
            event_key: "deploy-1".into(),
            data: None,
            data_json: Some("{\"ok\": true}".into()),
        };
        super::publish::run(args, client, OutputFormat::Json)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn search_logs_table_and_json() {
        let client = stub_client().await;
        let args = super::search_logs::SearchLogsArgs {
            query: "matched".into(),
            workflow: None,
            step: None,
            stream: None,
            limit: 50,
            skip: 0,
        };
        super::search_logs::run(args, client, OutputFormat::Table)
            .await
            .unwrap();

        let client = stub_client().await;
        let args = super::search_logs::SearchLogsArgs {
            query: "matched".into(),
            workflow: Some("wf-1".into()),
            step: Some("checkout".into()),
            stream: Some(super::search_logs::StreamFilter::Stdout),
            limit: 10,
            skip: 0,
        };
        super::search_logs::run(args, client, OutputFormat::Json)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn logs_streams_entries() {
        let client = stub_client().await;
        let args = super::logs::LogsArgs {
            workflow_id: "ci-42".into(),
            step: None,
            follow: false,
        };
        super::logs::run(&logger(), args, client).await.unwrap();
    }

    #[tokio::test]
    async fn watch_consumes_lifecycle_events() {
        let client = stub_client().await;
        let args = super::watch::WatchArgs {
            workflow_id: Some("ci-42".into()),
        };
        super::watch::run(args, client).await.unwrap();
    }
}
