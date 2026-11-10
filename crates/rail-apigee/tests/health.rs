use rail_apigee::{READY, Readiness, Statuses};
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_server::Health;

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

// A check made while draining, on a connection still open, mustn't bring readiness back.
#[tokio::test]
async fn once_stopped_both_stay_not_serving() {
    let (statuses, readiness) = Statuses::start(None).await;
    assert_eq!(check(&readiness, "").await, ServingStatus::Serving);
    assert_eq!(check(&readiness, READY).await, ServingStatus::Serving);

    statuses.stop().await;

    assert_eq!(check(&readiness, "").await, ServingStatus::NotServing);
    assert_eq!(check(&readiness, READY).await, ServingStatus::NotServing);
}
