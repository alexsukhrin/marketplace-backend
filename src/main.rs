use actix_web::{App, HttpServer, web};
use sqlx::postgres::PgPoolOptions;
use std::env;

mod handlers;
mod services;

use crate::handlers::auth;
use crate::handlers::products;
use crate::handlers::users;
use actix_cors::Cors;

use utoipa::OpenApi;
use utoipa_actix_web::{AppExt, scope as openapi_scope};
use utoipa_scalar::{Scalar, Servable as ScalarServable};
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    tags(
        (name = "Auth", description = "Register users.")
    )
)]
pub struct ApiDoc;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    println!("Starting server");

    dotenv::from_filename("env").ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new()
        .max_connections(5) // Максимальна кількість з'єднань
        .connect(&database_url)
        .await
        .expect("Failed to create pool.");

    HttpServer::new(move || {
        App::new()
            .into_utoipa_app()
            .openapi(ApiDoc::openapi())
            .map(|app| {
                app.wrap(
                    Cors::default()
                        .allow_any_origin() // або .allowed_origin("https://твій-домен")
                        .allow_any_method()
                        .allow_any_header(),
                )
            })
            .app_data(web::Data::new(pool.clone()))
            .service(
                openapi_scope("/api/v1"), // openapi service register
            )
            .openapi_service(|api| {
                SwaggerUi::new("/swagger-ui/{_:.*}").url("/api-docs/openapi.json", api)
            })
            .openapi_service(|api| Scalar::with_url("/scalar", api))
            .into_app()
            // TODO: Should be annotated with openapi specs and moved to the openapi service register
            .service(
                web::scope("/api/v1")
                    .service(
                        web::scope("/auth")
                            .service(auth::signup)
                            .service(auth::confirm)
                            .service(auth::login)
                            .service(auth::logout)
                            .service(auth::refresh_token)
                            .service(auth::reset_password)
                            .service(auth::otp_verify)
                            .service(auth::update_password),
                    )
                    .service(
                        web::scope("/users")
                            .service(users::create)
                            .service(users::categories),
                    )
                    .service(
                        web::scope("/products")
                            .service(products::categories)
                            .service(products::payment_options)
                            .service(products::delivery_options)
                            .service(products::create)
                            .service(products::get_products)
                            .service(products::get_colors)
                            .service(products::get_shoe_sizes)
                            .service(products::get_clothing_sizes)
                            .service(products::get_genders)
                            .service(products::get_materials),
                    ),
            )
    })
    .bind(("0.0.0.0", 4000))?
    .run()
    .await
}
