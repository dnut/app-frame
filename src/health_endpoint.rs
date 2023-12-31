use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use super::service_manager::ServiceManager;

pub async fn run(manager: Arc<ServiceManager>, config: HealthEndpointConfig) {
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let make_svc = make_service_fn(move |_conn| {
        let manager = manager.clone();
        let config = config.clone();
        async move {
            Ok::<_, Infallible>(service_fn(move |x| {
                health(manager.clone(), config.clone(), x)
            }))
        }
    });
    if let Err(e) = Server::bind(&addr).serve(make_svc).await {
        tracing::error!("health endpoint exited with error: {}", e);
    }
}

#[derive(Clone)]
pub struct HealthEndpointConfig {
    pub port: u16,
    pub success_status: StatusCode,
    pub fail_status: StatusCode,
}

impl Default for HealthEndpointConfig {
    fn default() -> Self {
        Self {
            port: 3417,
            success_status: StatusCode::OK,
            fail_status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

async fn health(
    manager: Arc<ServiceManager>,
    config: HealthEndpointConfig,
    req: Request<Body>,
) -> Result<Response<Body>, Infallible> {
    tracing::debug!("Received http request: {req:?}");
    let mut response = Response::new(Body::empty());
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/health") => {
            let status = manager.check();
            if status.dead.is_empty() {
                tracing::debug!("Responding 200 to health check.");
                *response.status_mut() = config.success_status;
                *response.body_mut() = format!("{} live services", status.alive.len()).into();
            } else {
                tracing::error!("Responding 500 to health check.");
                *response.status_mut() = config.fail_status;
                *response.body_mut() = format!(
                    "{} live services. dead services: {:#?}",
                    status.alive.len(),
                    status.dead
                )
                .into();
            }
        }
        _ => {
            tracing::info!("Responding 404 to unexpected request: {req:?}");
            *response.status_mut() = StatusCode::NOT_FOUND;
        }
    };
    Ok(response)
}
