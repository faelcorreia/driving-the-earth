//! Painel de hora do jogo (tecla T): data e hora locais de Belo Horizonte, campos para digitar,
//! velocidade e atalhos. Não pausa o jogo. Começa fechado e invisível, sem foco, então as teclas
//! do carro seguem sendo do carro. Regras de horário ficam em `time_ui`; aqui é só a tela.

use godot::builtin::Side;
use godot::classes::control::{LayoutPreset, SizeFlags};
use godot::classes::text_server::AutowrapMode;
use godot::classes::{
    Button, CanvasLayer, HBoxContainer, Label, LineEdit, OptionButton, Os, PanelContainer,
    VBoxContainer,
};
use godot::prelude::*;

use super::clock::EnvironmentClock;
use super::sky;
use super::time_ui::{self, Speed};
use crate::ui_text::{self, Lang};

/// Período de atualização do texto do painel quando aberto, em segundos reais.
const REFRESH_PERIOD_S: f64 = 0.25;
/// Largura do painel, em pixels de tela.
const PANEL_WIDTH_PX: f32 = 340.0;
/// Folga entre o painel e a borda superior direita da tela, em pixels.
const PANEL_MARGIN_PX: f32 = 16.0;
/// Camada do painel: acima do jogo e do mapa.
const PANEL_LAYER: i32 = 20;
/// Largura mínima do campo de data, em pixels.
const DATE_FIELD_WIDTH_PX: f32 = 110.0;
/// Largura mínima do campo de hora, em pixels.
const HOUR_FIELD_WIDTH_PX: f32 = 70.0;

/// Tela do painel de hora. Os sinais dos botões chamam métodos `#[func]` de `EnvironmentClock`,
/// que fazem a lógica e devolvem os valores para cá.
pub struct TimePanel {
    root: Gd<CanvasLayer>,
    date_edit: Gd<LineEdit>,
    hour_edit: Gd<LineEdit>,
    speed: Gd<OptionButton>,
    current: Gd<Label>,
    events: Gd<Label>,
    error: Gd<Label>,
    lang: Lang,
    open: bool,
    timer: f64,
    /// Dia civil local para o qual a linha de astros foi calculada (não recalcula no mesmo dia).
    events_day: Option<(i64, u32, u32)>,
}

impl TimePanel {
    /// Monta os nós do painel, ainda fora da árvore, e liga os botões ao relógio.
    pub fn build(clock: &Gd<EnvironmentClock>) -> TimePanel {
        let lang = ui_text::language(&Os::singleton().get_locale().to_string());

        let mut root = CanvasLayer::new_alloc();
        root.set_layer(PANEL_LAYER);
        root.set_visible(false);

        let mut panel = PanelContainer::new_alloc();
        panel.set_anchors_and_offsets_preset(LayoutPreset::TOP_RIGHT);
        panel.set_offset(Side::LEFT, -(PANEL_WIDTH_PX + PANEL_MARGIN_PX));
        panel.set_offset(Side::RIGHT, -PANEL_MARGIN_PX);
        panel.set_offset(Side::TOP, PANEL_MARGIN_PX);
        panel.set_custom_minimum_size(Vector2::new(PANEL_WIDTH_PX, 0.0));

        let mut column = VBoxContainer::new_alloc();
        let title = Self::label(&text("time.title", lang));
        let current = Self::label("");

        let mut date_row = HBoxContainer::new_alloc();
        let date_label = Self::label(&text("time.date_label", lang));
        let mut date_edit = LineEdit::new_alloc();
        date_edit.set_placeholder(&text("time.date_example", lang));
        date_edit.set_custom_minimum_size(Vector2::new(DATE_FIELD_WIDTH_PX, 0.0));
        date_edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
        let hour_label = Self::label(&text("time.hour_label", lang));
        let mut hour_edit = LineEdit::new_alloc();
        hour_edit.set_placeholder("HH:MM");
        hour_edit.set_custom_minimum_size(Vector2::new(HOUR_FIELD_WIDTH_PX, 0.0));
        let mut set_button = Button::new_alloc();
        set_button.set_text(&text("time.set", lang));
        date_row.add_child(&date_label);
        date_row.add_child(&date_edit);
        date_row.add_child(&hour_label);
        date_row.add_child(&hour_edit);
        date_row.add_child(&set_button);

        let mut speed_row = HBoxContainer::new_alloc();
        let speed_label = Self::label(&text("time.speed_label", lang));
        let mut speed = OptionButton::new_alloc();
        speed.set_h_size_flags(SizeFlags::EXPAND_FILL);
        for option in Speed::ALL {
            speed.add_item(&text(option.key(), lang));
        }
        speed_row.add_child(&speed_label);
        speed_row.add_child(&speed);

        let mut shortcut_row = HBoxContainer::new_alloc();
        let mut now_button = Button::new_alloc();
        now_button.set_text(&text("time.now", lang));
        let mut sunrise_button = Button::new_alloc();
        sunrise_button.set_text(&text("time.sunrise", lang));
        let mut sunset_button = Button::new_alloc();
        sunset_button.set_text(&text("time.sunset", lang));
        shortcut_row.add_child(&now_button);
        shortcut_row.add_child(&sunrise_button);
        shortcut_row.add_child(&sunset_button);

        let events = Self::label("");
        let mut error = Self::label("");
        error.set_visible(false);
        let hint = Self::label(&text("time.hint", lang));

        column.add_child(&title);
        column.add_child(&current);
        column.add_child(&date_row);
        column.add_child(&speed_row);
        column.add_child(&shortcut_row);
        column.add_child(&events);
        column.add_child(&error);
        column.add_child(&hint);
        panel.add_child(&column);
        root.add_child(&panel);

        // Os sinais chamam os `#[func]` do relógio, com o mesmo nome (ver `environment/clock.rs`).
        let on = |method: &str| Callable::from_object_method(clock, method);
        set_button.connect("pressed", &on("on_set_pressed"));
        now_button.connect("pressed", &on("on_now_pressed"));
        sunrise_button.connect("pressed", &on("on_sunrise_pressed"));
        sunset_button.connect("pressed", &on("on_sunset_pressed"));
        speed.connect("item_selected", &on("on_speed_selected"));
        date_edit.connect("text_submitted", &on("on_text_submitted"));
        hour_edit.connect("text_submitted", &on("on_text_submitted"));

        TimePanel {
            root,
            date_edit,
            hour_edit,
            speed,
            current,
            events,
            error,
            lang,
            open: false,
            timer: 0.0,
            events_day: None,
        }
    }

    /// Nó raiz (camada do painel), para adicionar à árvore.
    pub fn root(&self) -> Gd<CanvasLayer> {
        self.root.clone()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Mostra ou esconde. Ao fechar, solta o foco dos campos para o teclado voltar ao carro.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        self.root.set_visible(open);
        if !open {
            self.date_edit.release_focus();
            self.hour_edit.release_focus();
        }
    }

    /// Texto digitado nos campos de data e hora.
    pub fn typed(&self) -> (String, String) {
        (
            self.date_edit.get_text().to_string(),
            self.hour_edit.get_text().to_string(),
        )
    }

    /// Preenche os campos com a data e a hora locais de `unix` e esconde o erro.
    pub fn show_fields(&mut self, unix: f64) {
        let (y, m, d) = sky::local_date(unix);
        self.date_edit.set_text(&time_ui::format_date(y, m, d));
        self.hour_edit
            .set_text(&time_ui::format_hm(sky::local_hour(unix)));
        self.show_error(false);
    }

    /// Mostra ou esconde a mensagem de hora ou data inválida.
    pub fn show_error(&mut self, shown: bool) {
        self.error.set_text(&text("time.invalid", self.lang));
        self.error.set_visible(shown);
    }

    /// Atualiza o texto e o seletor de velocidade com o estado atual do relógio. Chamado a cada
    /// `REFRESH_PERIOD_S` com o painel aberto, e a linha de astros só recalcula ao mudar o dia.
    /// `delta` em segundos reais. Devolve `true` quando chegou a hora de atualizar.
    pub fn tick(&mut self, delta: f64) -> bool {
        if !self.open {
            return false;
        }
        self.timer += delta;
        if self.timer < REFRESH_PERIOD_S {
            return false;
        }
        self.timer = 0.0;
        true
    }

    /// Escreve o estado do relógio nos textos e no seletor de velocidade.
    pub fn update(&mut self, sim_unix: f64, speed_multiplier: f64) {
        let (y, m, d) = sky::local_date(sim_unix);
        let date = time_ui::format_date(y, m, d);
        let hour = time_ui::format_hm(sky::local_hour(sim_unix));
        self.current.set_text(&fmt(
            "time.current",
            self.lang,
            &[("date", &date), ("hour", &hour)],
        ));

        let index = Speed::nearest(speed_multiplier).index() as i32;
        if self.speed.get_selected() != index {
            self.speed.select(index);
        }

        if self.events_day != Some((y, m, d)) {
            self.events_day = Some((y, m, d));
            let line = self.events_line(sim_unix);
            self.events.set_text(&line);
        }
    }

    /// Linha discreta: nascer e pôr do sol do dia, fase e iluminação da lua.
    fn events_line(&self, sim_unix: f64) -> String {
        let (rise, set) = time_ui::day_events(sim_unix);
        let hour_of = |t: Option<f64>| match t {
            Some(t) => time_ui::format_hm(sky::local_hour(t)),
            None => text("time.no_event", self.lang).to_string(),
        };
        let moon = sky::moon(sim_unix);
        let phase = text(time_ui::moon_phase_key(&moon), self.lang);
        let percent = time_ui::moon_percent(&moon).to_string();
        fmt(
            "time.line",
            self.lang,
            &[
                ("rise", &hour_of(rise)),
                ("set", &hour_of(set)),
                ("moon", &phase),
                ("percent", &percent),
            ],
        )
    }

    fn label(text: &str) -> Gd<Label> {
        let mut label = Label::new_alloc();
        label.set_text(text);
        label.set_autowrap_mode(AutowrapMode::WORD_SMART);
        label
    }
}

/// Texto de `key` no idioma do painel.
fn text(key: &str, lang: Lang) -> String {
    ui_text::text(key, lang).to_string()
}

/// Texto de `key` com os `{nome}` trocados pelos valores.
fn fmt(key: &str, lang: Lang, values: &[(&str, &str)]) -> String {
    let mut out = ui_text::text(key, lang).to_string();
    for (name, value) in values {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}
