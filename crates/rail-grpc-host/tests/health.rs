use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rail_grpc_host::{
    Address, READY, Readiness, Role, RoleCheck, RoleState, Statuses, ask_health, breakdown_header,
};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::metadata::MetadataValue;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_server::{Health, HealthServer};
use tonic_health::pb::{HealthCheckRequest, HealthCheckResponse};

async fn check(readiness: &Readiness, service: &str) -> ServingStatus {
    let request = tonic::Request::new(HealthCheckRequest {
        service: service.to_owned(),
    });
    readiness
        .check(request)
        .await
        .unwrap()
        .into_inner()
        .status()
}

// A role whose readiness the test sets, as "up" or "down".
fn switch(ready: bool) -> (Arc<AtomicBool>, RoleCheck) {
    let switch = Arc::new(AtomicBool::new(ready));
    let read = Arc::clone(&switch);
    let check: RoleCheck = Arc::new(move || {
        let ready = read.load(Ordering::SeqCst);
        RoleState {
            ready,
            state: if ready { "up" } else { "down" },
        }
    });
    (switch, check)
}

#[tokio::test]
async fn liveness_answers_for_the_server_and_each_service_named() {
    let (_statuses, readiness) = Statuses::start(
        &["a.Service"],
        vec![(Role::Proxy, Arc::new(|| RoleState::DISABLED))],
    )
    .await;

    assert_eq!(check(&readiness, "").await, ServingStatus::Serving);
    assert_eq!(check(&readiness, "a.Service").await, ServingStatus::Serving);
    assert!(
        readiness
            .check(tonic::Request::new(HealthCheckRequest {
                service: "b.Service".to_owned(),
            }))
            .await
            .is_err()
    );
}

// Read on each check, so it follows the role both ways; liveness never does.
#[tokio::test]
async fn readiness_follows_the_role_on_each_check() {
    let (switch, role) = switch(false);
    let (statuses, readiness) = Statuses::start(&[], vec![(Role::Proxy, role)]).await;

    assert_eq!(check(&readiness, READY).await, ServingStatus::NotServing);
    assert_eq!(check(&readiness, "").await, ServingStatus::Serving);
    assert_eq!(
        statuses.read_roles(),
        [(
            Role::Proxy,
            RoleState {
                ready: false,
                state: "down"
            }
        )]
    );

    switch.store(true, Ordering::SeqCst);
    assert_eq!(check(&readiness, READY).await, ServingStatus::Serving);

    switch.store(false, Ordering::SeqCst);
    assert_eq!(check(&readiness, READY).await, ServingStatus::NotServing);
    assert_eq!(check(&readiness, "").await, ServingStatus::Serving);
}

#[tokio::test]
async fn ready_only_while_every_role_is() {
    let (proxy, proxy_check) = switch(true);
    let (_gateway, gateway_check) = switch(false);
    let (statuses, readiness) = Statuses::start(
        &[],
        vec![(Role::Proxy, proxy_check), (Role::Gateway, gateway_check)],
    )
    .await;

    assert_eq!(check(&readiness, READY).await, ServingStatus::NotServing);
    assert_eq!(
        statuses.read_roles(),
        [
            (
                Role::Proxy,
                RoleState {
                    ready: true,
                    state: "up"
                }
            ),
            (
                Role::Gateway,
                RoleState {
                    ready: false,
                    state: "down"
                }
            ),
        ]
    );
    proxy.store(false, Ordering::SeqCst);
    assert_eq!(check(&readiness, READY).await, ServingStatus::NotServing);
}

// A check made while draining, on a connection still open, mustn't bring readiness back.
#[tokio::test]
async fn once_stopped_every_service_stays_not_serving() {
    let (_switch, role) = switch(true);
    let (statuses, readiness) = Statuses::start(&["a.Service"], vec![(Role::Gateway, role)]).await;
    assert_eq!(check(&readiness, READY).await, ServingStatus::Serving);

    statuses.stop().await;

    for service in ["", "a.Service", READY] {
        assert_eq!(
            check(&readiness, service).await,
            ServingStatus::NotServing,
            "{service}"
        );
    }
}

// Readiness and its breakdown, from one check; liveness gives none.
#[tokio::test]
async fn readiness_gives_each_roles_state_in_its_metadata() {
    let (_proxy, proxy_check) = switch(true);
    let (_gateway, gateway_check) = switch(false);
    let (statuses, readiness) = Statuses::start(
        &[],
        vec![(Role::Proxy, proxy_check), (Role::Gateway, gateway_check)],
    )
    .await;
    let ask = |service: &str| {
        readiness.check(tonic::Request::new(HealthCheckRequest {
            service: service.to_owned(),
        }))
    };

    let ready = ask(READY).await.unwrap();
    let live = ask("").await.unwrap();
    statuses.stop().await;
    let stopped = ask(READY).await.unwrap();

    let header = |response: &tonic::Response<HealthCheckResponse>, role| {
        let value = response.metadata().get(breakdown_header(role).as_str());
        value.map(|value| value.to_str().unwrap().to_owned())
    };
    assert_eq!(header(&ready, Role::Proxy).as_deref(), Some("up"));
    assert_eq!(header(&ready, Role::Gateway).as_deref(), Some("down"));
    assert_eq!(header(&live, Role::Proxy), None);
    assert_eq!(header(&stopped, Role::Proxy).as_deref(), Some("stopping"));
    assert_eq!(header(&stopped, Role::Gateway).as_deref(), Some("stopping"));
}

async fn serve(service: HealthServer<impl Health>) -> Address {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = Address::Tcp(listener.local_addr().unwrap());
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    address
}

#[tokio::test]
async fn the_probe_prints_the_breakdown_after_the_status_by_role() {
    let (switch, proxy_check) = switch(true);
    let gateway_check: RoleCheck = Arc::new(|| RoleState {
        ready: true,
        state: "held",
    });
    let (_statuses, readiness) = Statuses::start(
        &[],
        vec![(Role::Proxy, proxy_check), (Role::Gateway, gateway_check)],
    )
    .await;
    let address = serve(HealthServer::new(readiness)).await;

    assert_eq!(
        ask_health(&address, READY).await,
        Ok("serving (gateway: held, proxy: up)".to_owned())
    );
    switch.store(false, Ordering::SeqCst);
    assert_eq!(
        ask_health(&address, READY).await,
        Err("not serving: NOT_SERVING (gateway: held, proxy: down)".to_owned())
    );
    assert_eq!(ask_health(&address, "").await, Ok("serving".to_owned()));
}

// What a server sends is shown made safe, as any wire value is.
struct Hostile;

#[tonic::async_trait]
impl Health for Hostile {
    async fn check(
        &self,
        _request: tonic::Request<HealthCheckRequest>,
    ) -> Result<tonic::Response<HealthCheckResponse>, tonic::Status> {
        let mut response = tonic::Response::new(HealthCheckResponse {
            status: ServingStatus::Serving as i32,
        });
        let value = MetadataValue::try_from("\u{202e}held".as_bytes()).unwrap();
        response.metadata_mut().insert("rail-ready-proxy", value);
        Ok(response)
    }

    type WatchStream = <Readiness as Health>::WatchStream;

    async fn watch(
        &self,
        _request: tonic::Request<HealthCheckRequest>,
    ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("watch"))
    }
}

#[tokio::test]
async fn the_probe_sanitizes_the_breakdown() {
    let address = serve(HealthServer::new(Hostile)).await;

    assert_eq!(
        ask_health(&address, READY).await,
        Ok("serving (proxy: \\u{202e}held)".to_owned())
    );
}
