//! Formato do mapa salvo em disco. Compartilhado entre o bake e o leitor do jogo.
use serde::{Deserialize, Serialize};

/// Conteúdo de `cache/<id>/meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapMeta {
    pub id: String,
    pub name: String,
    pub center_lat: f64,
    pub center_lon: f64,
    pub radius_m: f64,
    pub tile_size_m: f64,
    pub created_unix: u64,
    pub complete: bool,
}
