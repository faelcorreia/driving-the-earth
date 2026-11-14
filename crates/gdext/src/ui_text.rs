//! Player-visible text, by key (en and pt-BR). Pure: no Godot types. Code asks for a key and the
//! locale of the game; the English text is the fallback for any other locale.

/// Languages with translated text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    PtBr,
}

/// Language for an OS locale such as `pt_BR` or `en_US`; anything else is English.
pub fn language(locale: &str) -> Lang {
    if locale.len() >= 2 && locale[..2].eq_ignore_ascii_case("pt") {
        Lang::PtBr
    } else {
        Lang::En
    }
}

/// Text of `key` in `lang`. An unknown key returns the key itself, so a missing entry shows up
/// on screen instead of failing silently.
pub fn text(key: &str, lang: Lang) -> &str {
    let entry = TABLE.iter().find(|(k, _, _)| *k == key);
    match (entry, lang) {
        (Some((_, _, pt)), Lang::PtBr) => pt,
        (Some((_, en, _)), Lang::En) => en,
        (None, _) => key,
    }
}

/// Text key of a failed bake, from the failure `kind` and `message` the bake reports (`BakeJob`).
/// The cause is read from the message, which `tilebake` and `overpass` build:
/// - an Overpass answer 429, 502, 503 or 504 (busy, rate-limited or down) is `overpass`;
/// - a transport error (DNS, connection, timeout: no network) is `network`;
/// - a file error (no space, no permission, read-only) is `disk`;
/// - anything else, and an invalid request, is the generic `home.bake_failed`.
pub fn bake_failure_key(kind: &str, message: &str) -> &'static str {
    if kind == "invalid_input" {
        return "home.bake_failed";
    }
    let text = message.to_ascii_lowercase();
    // Checked before the transport words: an Overpass answer also says "HTTP".
    if ["http 429", "http 502", "http 503", "http 504"]
        .iter()
        .any(|code| text.contains(code))
    {
        return "home.bake_fail.overpass";
    }
    if text.contains("http ") {
        return "home.bake_failed";
    }
    const NETWORK: [&str; 8] = [
        "dns",
        "connection",
        "connect",
        "timed out",
        "timeout",
        "unreachable",
        "resolve",
        "network",
    ];
    if NETWORK.iter().any(|word| text.contains(word)) {
        return "home.bake_fail.network";
    }
    const DISK: [&str; 7] = [
        "no space left",
        "disk quota",
        "permission denied",
        "read-only file system",
        "os error 28",
        "os error 13",
        "os error 30",
    ];
    if DISK.iter().any(|word| text.contains(word)) {
        return "home.bake_fail.disk";
    }
    "home.bake_failed"
}

/// Text key of a bake stage (the stage keys `tilebake` reports). A stage the table does not know
/// gets the generic line, so a new stage shows something instead of its raw key.
pub fn bake_stage_key(stage: &str) -> &'static str {
    match stage {
        tilebake::STAGE_OSM => "bake.stage.osm",
        tilebake::STAGE_OSM_AREAS => "bake.stage.osm_areas",
        tilebake::STAGE_DEM => "bake.stage.dem",
        tilebake::STAGE_TILES => "bake.stage.tiles",
        tilebake::STAGE_ROADS => "bake.stage.roads",
        tilebake::STAGE_FAR_LIGHTS => "bake.stage.far_lights",
        _ => "bake.stage.other",
    }
}

/// Every key with its English and pt-BR text.
const TABLE: &[(&str, &str, &str)] = &[
    (
        "worldmap.hint",
        "M or Esc closes  |  wheel: zoom  |  drag: move  |  click: choose a place  |  Confirm: go",
        "M ou Esc fecha  |  roda: zoom  |  arrastar: mover  |  clique: escolher um lugar  |  Confirmar: ir",
    ),
    ("time.title", "Game time", "Hora do jogo"),
    (
        "time.current",
        "Game: {date} {hour} (BRT)",
        "Jogo: {date} {hour} (BRT)",
    ),
    ("time.day", "Day", "Dia"),
    ("time.month", "Month", "Mês"),
    ("time.year", "Year", "Ano"),
    ("time.hour", "Hour", "Hora"),
    ("time.minute", "Minute", "Minuto"),
    ("time.decrease", "Decrease {field}", "Diminuir {field}"),
    ("time.increase", "Increase {field}", "Aumentar {field}"),
    ("time.set", "Set", "Aplicar"),
    (
        "time.invalid",
        "Invalid hour or date",
        "Hora ou data inválida",
    ),
    ("time.speed_label", "Speed", "Velocidade"),
    ("time.speed.fixed", "Fixed", "Fixa"),
    ("time.speed.real", "1x real", "1x tempo real"),
    ("time.speed.fast", "Fast (60x)", "Rápido (60x)"),
    ("time.now", "Now", "Agora"),
    ("time.sunrise", "Sunrise", "Nascer do sol"),
    ("time.sunset", "Sunset", "Pôr do sol"),
    ("time.midnight", "Midnight", "Meia-noite"),
    ("time.full_moon", "Full moon", "Lua cheia"),
    ("time.new_moon", "New moon", "Lua nova"),
    (
        "time.line",
        "Sunrise {rise}  ·  Sunset {set}  ·  Moon {moon} {percent}%",
        "Nascer {rise}  ·  Pôr {set}  ·  Lua {moon} {percent}%",
    ),
    ("time.no_event", "no event", "sem evento"),
    ("time.moon.new", "new", "nova"),
    ("time.moon.waxing_crescent", "waxing crescent", "crescente"),
    (
        "time.moon.waxing_gibbous",
        "waxing gibbous",
        "crescente gibosa",
    ),
    ("time.moon.full", "full", "cheia"),
    (
        "time.moon.waning_gibbous",
        "waning gibbous",
        "minguante gibosa",
    ),
    ("time.moon.waning_crescent", "waning crescent", "minguante"),
    (
        "time.hint",
        "Tab: next field  |  Up, Down: change  |  Left, Right: minus, plus  |  T or Esc closes",
        "Tab: próximo campo  |  cima, baixo: mudar  |  esquerda, direita: menos, mais  |  T ou Esc fecha",
    ),
    (
        "home.hint",
        "Wheel: zoom  |  drag: move  |  click: choose a place",
        "Roda: zoom  |  arrastar: mover  |  clique: escolher um lugar",
    ),
    ("home.confirm", "Confirm", "Confirmar"),
    (
        "home.pick_first",
        "Click on the map to choose your home",
        "Clique no mapa para escolher sua casa",
    ),
    ("home.selected", "Selected: {lat}, {lon}", "Escolhido: {lat}, {lon}"),
    ("home.current", "Current home: {lat}, {lon}", "Casa atual: {lat}, {lon}"),
    ("home.saved", "Home saved: {lat}, {lon}", "Casa salva: {lat}, {lon}"),
    (
        "home.save_error",
        "Could not save your home",
        "Não foi possível salvar sua casa",
    ),
    (
        "home.baking",
        "This place has no map yet: preparing it. The first visit takes a while.",
        "Este lugar ainda não tem mapa: preparando. A primeira visita leva um tempo.",
    ),
    (
        "home.bake_busy",
        "This place is already being prepared.",
        "Este lugar já está sendo preparado.",
    ),
    (
        "home.bake_cancelled",
        "Download cancelled: choose another place (M).",
        "Download cancelado: escolha outro lugar (M).",
    ),
    (
        "home.bake_failed",
        "Could not prepare this place: choose another one (M).",
        "Não foi possível preparar este lugar: escolha outro (M).",
    ),
    (
        "home.bake_fail.network",
        "No internet connection: the first visit to a place needs it. Try again when you are online.",
        "Sem conexão com a internet: a primeira visita a um lugar precisa dela. Tente de novo quando estiver online.",
    ),
    (
        "home.bake_fail.overpass",
        "The OpenStreetMap server is busy or down. Try again in a few minutes.",
        "O servidor do OpenStreetMap está ocupado ou fora do ar. Tente de novo em alguns minutos.",
    ),
    (
        "home.bake_fail.disk",
        "Could not save the map: check the free disk space and the game folder's permissions.",
        "Não foi possível salvar o mapa: confira o espaço livre em disco e as permissões da pasta do jogo.",
    ),
    ("bake.retry", "Try again", "Tentar de novo"),
    ("bake.back", "Back", "Voltar"),
    (
        "home.bake_no_road",
        "No road found for the car at this place.",
        "Não encontrei uma via para o carro neste lugar.",
    ),
    ("bake.cancel", "Cancel", "Cancelar"),
    (
        "bake.stage.osm",
        "Downloading streets and buildings",
        "Baixando ruas e prédios",
    ),
    (
        "bake.stage.osm_areas",
        "Downloading parks and water",
        "Baixando parques e água",
    ),
    ("bake.stage.dem", "Downloading the terrain", "Baixando o relevo"),
    (
        "bake.stage.tiles",
        "Building the blocks",
        "Montando os quarteirões",
    ),
    ("bake.stage.roads", "Building the roads", "Montando as vias"),
    (
        "bake.stage.far_lights",
        "Building the distant lights",
        "Montando as luzes distantes",
    ),
    ("bake.stage.other", "Preparing", "Preparando"),
    (
        "home.none_start",
        "No home saved yet: choose one on the map (M).",
        "Nenhuma casa salva ainda: escolha uma no mapa (M).",
    ),
    (
        "home.outside_saved",
        "Your saved home is outside the loaded map: choose a place (M).",
        "Sua casa salva está fora do mapa carregado: escolha um lugar (M).",
    ),
    (
        "home.no_road_start",
        "No road near your saved home: choose a place (M).",
        "Nenhuma via perto da sua casa salva: escolha um lugar (M).",
    ),
    (
        "home.no_map",
        "No map data for the game yet: choose a place on the map (M).",
        "Ainda não há dados de mapa para o jogo: escolha um lugar no mapa (M).",
    ),
    (
        "home.map_missing",
        "World map data is missing",
        "Dados do mapa-múndi não encontrados",
    ),
    (
        "home.attribution",
        "Map data: Natural Earth (public domain). \u{a9} OpenStreetMap contributors, OpenFreeMap",
        "Dados do mapa: Natural Earth (domínio público). \u{a9} OpenStreetMap contributors, OpenFreeMap",
    ),
    (
        "home.detail_offline",
        "Streets and neighborhoods need the internet. Showing cities only.",
        "Ruas e bairros precisam de internet. Mostrando só as cidades.",
    ),
    ("hud.speed_unit", "km/h", "km/h"),
    ("loading.title", "Loading", "Carregando"),
    ("loading.opening_map", "Opening the map", "Abrindo o mapa"),
    ("loading.reading_map", "Reading the map", "Lendo o mapa"),
    (
        "loading.building_tiles",
        "Building the first blocks",
        "Montando o terreno inicial",
    ),
    ("loading.placing_car", "Placing the car", "Posicionando o carro"),
    (
        "loading.environment",
        "Turning on the lights",
        "Acendendo as luzes",
    ),
    (
        "loading.world_map",
        "Preparing the world map",
        "Preparando o mapa-múndi",
    ),
    ("loading.ready", "Ready", "Pronto"),
    (
        "loading.hint",
        "The first blocks load around the car.",
        "Os primeiros quarteirões carregam ao redor do carro.",
    ),
    (
        "loading.error",
        "Could not load this map",
        "Não foi possível carregar este mapa",
    ),
    (
        "style.missing",
        "Style '{id}' not found: using '{fallback}'",
        "Estilo '{id}' não encontrado: usando '{fallback}'",
    ),
    ("style.active", "Style: {id}", "Estilo: {id}"),
    ("intro.title", "Driving the Earth", "Driving the Earth"),
    (
        "intro.hint",
        "Any key, click or button skips  |  starts on its own in 3 s",
        "Qualquer tecla, clique ou botão pula  |  começa sozinho em 3 s",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_both_languages() {
        for (key, en, pt) in TABLE {
            assert!(!en.is_empty() && !pt.is_empty(), "{key}");
            // Formats such as HH:MM and units such as km/h are the same in both languages, and so
            // is the game's name, a proper noun.
            if !key.ends_with("_example") && !key.ends_with("_unit") && *key != "intro.title" {
                assert_ne!(en, pt, "{key} is not translated");
            }
            assert_eq!(text(key, Lang::En), *en);
            assert_eq!(text(key, Lang::PtBr), *pt);
        }
        let mut keys: Vec<&str> = TABLE.iter().map(|(k, _, _)| *k).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), TABLE.len(), "duplicate key");
    }

    #[test]
    fn every_bake_stage_has_a_translated_line() {
        let stages = [
            tilebake::STAGE_OSM,
            tilebake::STAGE_OSM_AREAS,
            tilebake::STAGE_DEM,
            tilebake::STAGE_TILES,
            tilebake::STAGE_ROADS,
            tilebake::STAGE_FAR_LIGHTS,
            "some_future_stage",
        ];
        for stage in stages {
            let key = bake_stage_key(stage);
            assert!(TABLE.iter().any(|(k, _, _)| *k == key), "{stage} -> {key}");
            assert_ne!(text(key, Lang::PtBr), key, "{stage} has no pt-BR text");
        }
        assert_eq!(bake_stage_key("unknown"), "bake.stage.other");
    }

    #[test]
    fn bake_failures_map_to_their_cause() {
        // No network: transport errors of Overpass (prefixed) and of Copernicus (bare).
        let dns = "Overpass request failed: Dns Failed: resolve dns name 'overpass.invalid:443'";
        assert_eq!(bake_failure_key("other", dns), "home.bake_fail.network");
        let connect = "Overpass request failed: Connection Failed: tcp connect error";
        assert_eq!(bake_failure_key("other", connect), "home.bake_fail.network");
        let timeout = "Overpass request failed: Connection timed out";
        assert_eq!(bake_failure_key("other", timeout), "home.bake_fail.network");
        assert_eq!(
            bake_failure_key("other", "Dns Failed: no address"),
            "home.bake_fail.network"
        );

        // Overpass busy, rate-limited or down, with the retries spent.
        for code in [429, 502, 503, 504] {
            let msg = format!("Overpass request failed: HTTP {code}");
            assert_eq!(
                bake_failure_key("other", &msg),
                "home.bake_fail.overpass",
                "{code}"
            );
        }
        // Other HTTP answers are not an outage: the generic line.
        assert_eq!(
            bake_failure_key("other", "Overpass request failed: HTTP 400"),
            "home.bake_failed"
        );
        assert_eq!(
            bake_failure_key("other", "http status: 404"),
            "home.bake_failed"
        );

        // Disk, from the OS messages.
        assert_eq!(
            bake_failure_key("other", "No space left on device (os error 28)"),
            "home.bake_fail.disk"
        );
        assert_eq!(
            bake_failure_key("other", "Permission denied (os error 13)"),
            "home.bake_fail.disk"
        );

        // The rest, and an invalid request, keep the generic line.
        assert_eq!(
            bake_failure_key("other", "bake thread ended without a result"),
            "home.bake_failed"
        );
        assert_eq!(
            bake_failure_key("invalid_input", "Dns Failed"),
            "home.bake_failed"
        );
        assert_eq!(
            bake_failure_key("other", "unexpected layout"),
            "home.bake_failed"
        );
    }

    #[test]
    fn failure_and_retry_keys_are_translated() {
        for key in [
            "home.bake_failed",
            "home.bake_fail.network",
            "home.bake_fail.overpass",
            "home.bake_fail.disk",
            "bake.retry",
            "bake.back",
        ] {
            assert!(TABLE.iter().any(|(k, _, _)| *k == key), "{key}");
            assert_ne!(text(key, Lang::PtBr), key, "{key}");
        }
    }

    #[test]
    fn locale_picks_the_language_and_unknown_keys_show_themselves() {
        assert_eq!(language("pt_BR"), Lang::PtBr);
        assert_eq!(language("pt"), Lang::PtBr);
        assert_eq!(language("en_US"), Lang::En);
        assert_eq!(language("es_ES"), Lang::En);
        assert_eq!(language(""), Lang::En);
        assert_eq!(text("map.missing", Lang::PtBr), "map.missing");
    }
}
