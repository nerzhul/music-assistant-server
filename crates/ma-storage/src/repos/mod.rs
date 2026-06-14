//! Repositories — typed query helpers built on top of `sqlx::AnyPool`.

pub mod auth_repo;
pub mod cover_art_repo;

pub use auth_repo::AuthRepository;
pub use cover_art_repo::CoverArtRepository;
