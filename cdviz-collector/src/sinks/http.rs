use cdevents_sdk::cloudevents::BuilderExt;
use cloudevents::{EventBuilder, EventBuilderV10};
use reqwest::Url;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use serde::{Deserialize, Serialize};
//use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use crate::errors::Result;
use crate::Message;
use reqwest_tracing::TracingMiddleware;

use super::Sink;

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Config {
    destination: Url,
}

impl TryFrom<Config> for HttpSink {
    type Error = crate::errors::Error;

    fn try_from(value: Config) -> Result<Self> {
        Ok(HttpSink::new(value.destination))
    }
}

#[derive(Debug)]
pub(crate) struct HttpSink {
    client: ClientWithMiddleware,
    dest: Url,
}

impl HttpSink {
    pub(crate) fn new(url: Url) -> Self {
        // Retry up to 3 times with increasing intervals between attempts.
        //let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let client = ClientBuilder::new(reqwest::Client::new())
            // Trace HTTP requests. See the tracing crate to make use of these traces.
            .with(TracingMiddleware::default())
            // Retry failed requests.
            //.with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();
        Self { dest: url, client }
    }
}

impl Sink for HttpSink {
    //TODO use cloudevents
    async fn send(&self, msg: &Message) -> Result<()> {
        let cd_event = msg.cdevent.clone();
        // convert  CdEvent to cloudevents
        let event_result = EventBuilderV10::new().with_cdevent(cd_event.clone());
        match event_result {
            Ok(event_builder) => {
                let event_result = event_builder.build();
                let value = event_result?;
                
                //TODO use self.client (with middleware)
                self.client
                    .post(self.dest.clone())
                    .json(&value)
                    //.map_err(|e| tracing::warn!(error = ?e, "Failed to build request-builder"))
                    //.header("Access-Control-Allow-Origin", "*")
                    .send()
                    .await?;
            }
            Err(err) => {
                tracing::warn!(error = ?err, "Failed to convert to cloudevents");
                // In error case, send the original event
                self.client
                    .post(self.dest.clone())
                    .json(&cd_event)
                    .send()
                    .await?;
            }
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_test::{TestServer, TestServerConfig};
    use rstest::*;
    use rustainers::images::Postgres;
    use rustainers::runner::{RunOption, Runner};
    use rustainers::Container;
    use serde_json::json;

    struct TestContext {
        http: TestServer,
        // keep db container to drop it after the test
        _db_guard: Container<Postgres>,
        // keep tracing subscriber
        _tracing_guard: tracing::subscriber::DefaultGuard,
    }

    // #[fixture]
    // //#[once] // only work with non-async, non generic fixtures
    // // workaround explained at [Async once fixtures · Issue #141 · la10736/rstest](https://github.com/la10736/rstest/issues/141)
    // // no drop call on the fixture like on static
    // fn pg() -> (PgPool, Container<Postgres>) {
    //     futures::executor::block_on(async { async_pg().await })
    // }

    // servers() is called once per test, so db could only started several times.
    // We could not used `static` (or the once on fixtures) because statis are not dropped at end of the test
    #[fixture]
    async fn testcontext(#[future] async_pg: (PgPool, Container<Postgres>)) -> TestContext {
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::WARN)
            .finish();
        let _tracing_guard = tracing::subscriber::set_default(subscriber);

        let (pg_pool, db) = async_pg.await;
        let app_state = AppState {
            pg_pool: Arc::new(pg_pool.clone()),
        };
        let app = app().with_state(app_state);

        let config = TestServerConfig::builder()
            // Preserve cookies across requests
            // for the session cookie to work.
            .save_cookies()
            .expect_success_by_default()
            .mock_transport()
            .build();

        TestContext {
            http: TestServer::new_with_config(app, config).unwrap(),
            _db_guard: db,
            _tracing_guard,
        }
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    // test health endpoint
    async fn test_readyz(#[future] testcontext: TestContext) {
        let resp = testcontext.await.http.get("/readyz").await;
        resp.assert_status_ok();
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_post_cdevents(#[future] testcontext: TestContext) {
        let resp = testcontext
            .await
            .http
            .post("/cdevents")
            .json(&json!({
                "bar": "foo",
            }))
            .await;
        resp.assert_text("");
        resp.assert_status(http::StatusCode::CREATED);
    }
}
