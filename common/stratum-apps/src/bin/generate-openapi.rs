use stratum_apps::monitoring::http_server::ApiDoc;
use utoipa::OpenApi;

fn main() {
    let spec = ApiDoc::openapi();
    println!("{}", spec.to_json().unwrap());
}
