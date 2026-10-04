use tokio::net::TcpListener;

use __app___web::{app, config::Config};

/// The socket `cargo griffin dev` handed over, if this is a dev build it started;
/// otherwise a new one on the configured port.
async fn listener(port: u16) -> std::io::Result<TcpListener> {
    #[cfg(feature = "dev")]
    if let Some(listener) = griffin::dev::inherited_listener()? {
        return Ok(listener);
    }
    TcpListener::bind(("127.0.0.1", port)).await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Fails at boot, naming the setting, if the configuration is incomplete.
    let config = Config::load()?;
    // Under `cargo griffin dev` the asset tools are still building them at this point.
    #[cfg(not(feature = "dev"))]
    __app___web::assets::check()?;
    let listener = listener(config.port).await?;
    println!("__App__ at http://{}", listener.local_addr()?);
    griffin::axum::serve(listener, app(&config)?).await?;
    Ok(())
}
