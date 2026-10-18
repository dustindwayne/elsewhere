use elsewhere_core::{network::Runtime,store::Store};
use std::path::PathBuf;
#[tokio::main]
async fn main(){
 let dir=std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(||if DESKTOP_SERVER{std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("com.dustindwayne.elsewhere")}else{PathBuf::from("data/preview")});
 let runtime=Runtime::new(Store::open_secure(&dir).expect("Could not unlock storage"));
 elsewhere_core::agents::resume(&runtime).await.expect("Could not resume agent connections");
 let web=elsewhere_core::web::start(&runtime,false).await.expect("Could not start web backend");
 println!("Elsewhere web backend: {}",web["url"].as_str().unwrap());
 std::future::pending::<()>().await;
}
