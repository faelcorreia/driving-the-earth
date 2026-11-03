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
    ("map.loading", "Loading map...", "Carregando mapa..."),
    (
        "map.hint",
        "M close  |  wheel: zoom  |  drag: move",
        "M fecha  |  roda: zoom  |  arrastar: mover",
    ),
    (
        "map.attribution",
        "Map data © OpenStreetMap contributors",
        "Dados do mapa © colaboradores do OpenStreetMap",
    ),
    (
        "map.no_roads",
        "No roads in this map",
        "Nenhuma via neste mapa",
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
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_both_languages() {
        for (key, en, pt) in TABLE {
            assert!(!en.is_empty() && !pt.is_empty(), "{key}");
            assert_ne!(en, pt, "{key} is not translated");
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
