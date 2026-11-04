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

/// Every key with its English and pt-BR text.
const TABLE: &[(&str, &str, &str)] = &[
    (
        "worldmap.hint",
        "M or Esc closes  |  wheel: zoom  |  drag: move  |  click: place home  |  Confirm saves",
        "M ou Esc fecha  |  roda: zoom  |  arrastar: mover  |  clique: marcar casa  |  Confirmar salva",
    ),
    ("time.title", "Game time", "Hora do jogo"),
    (
        "time.current",
        "Game: {date} {hour} (BRT)",
        "Jogo: {date} {hour} (BRT)",
    ),
    ("time.date_label", "Date", "Data"),
    ("time.hour_label", "Hour", "Hora"),
    ("time.date_example", "DD/MM/YYYY", "DD/MM/AAAA"),
    ("time.hour_example", "HH:MM", "HH:MM"),
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
        "T or Esc closes the panel",
        "T ou Esc fecha o painel",
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
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_both_languages() {
        for (key, en, pt) in TABLE {
            assert!(!en.is_empty() && !pt.is_empty(), "{key}");
            // Formats such as HH:MM are the same in both languages.
            if !key.ends_with("_example") {
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
    fn locale_picks_the_language_and_unknown_keys_show_themselves() {
        assert_eq!(language("pt_BR"), Lang::PtBr);
        assert_eq!(language("pt"), Lang::PtBr);
        assert_eq!(language("en_US"), Lang::En);
        assert_eq!(language("es_ES"), Lang::En);
        assert_eq!(language(""), Lang::En);
        assert_eq!(text("map.missing", Lang::PtBr), "map.missing");
    }
}
