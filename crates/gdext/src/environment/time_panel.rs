//! Painel de hora do jogo (tecla T): data e hora locais de Belo Horizonte, escolhidas com botões
//! de passo (sem digitar texto), velocidade e atalhos. Não pausa o jogo. Começa fechado e
//! invisível, sem foco, então as teclas do carro seguem sendo do carro. Ao abrir, o foco cai no
//! primeiro campo; com foco num campo, cima e baixo mudam o valor (o `clock` faz esse passo antes
//! do foco). Regras de horário ficam em `time_ui`; aqui é só a tela.

use godot::builtin::Side;
use godot::classes::control::{LayoutPreset, SizeFlags};
use godot::classes::text_server::AutowrapMode;
use godot::classes::{
    Button, CanvasLayer, GridContainer, HBoxContainer, Label, MarginContainer, OptionButton, Os,
    PanelContainer, StyleBoxFlat, Theme, VBoxContainer,
};
use godot::global::HorizontalAlignment;
use godot::obj::InstanceId;
use godot::prelude::*;

use super::clock::EnvironmentClock;
use super::sky;
use super::time_ui::{self, Field, Picked, Speed};
use crate::style;
use crate::ui_text::{self, Lang};

/// Período de atualização do texto do painel quando aberto, em segundos reais.
const REFRESH_PERIOD_S: f64 = 0.25;
/// Largura do painel, em pixels de tela. Três campos de data lado a lado precisam dela.
const PANEL_WIDTH_PX: f32 = 380.0;
/// Folga entre o painel e a borda superior direita da tela, em pixels.
const PANEL_MARGIN_PX: f32 = 16.0;
/// Camada do painel: acima do jogo e do mapa.
const PANEL_LAYER: i32 = 20;
/// Folga interna entre a borda do painel e o conteúdo, em pixels.
const PANEL_PADDING_PX: i32 = 10;
/// Largura do conteúdo (painel menos a folga dos dois lados), em pixels. Os textos que quebram
/// linha precisam dela, senão um `Label` com quebra encolhe a zero e quebra letra por letra.
const CONTENT_WIDTH_PX: f32 = PANEL_WIDTH_PX - 2.0 * PANEL_PADDING_PX as f32;
/// Largura mínima do valor de um campo, em pixels, para o número não pular ao mudar.
const VALUE_WIDTH_PX: f32 = 48.0;
/// Inner padding of a themed button, in pixels (layout, not a theme field).
const BUTTON_PADDING_PX: f32 = 4.0;
/// Thickness of the focus ring, in pixels: at least this, even when the theme border is thinner.
const FOCUS_RING_PX: i32 = 3;

/// Um campo de data ou hora: botões de menos e de mais e o valor no meio.
struct Stepper {
    field: Field,
    minus: Gd<Button>,
    plus: Gd<Button>,
    value: Gd<Label>,
}

/// Tela do painel de hora. Os botões de passo chamam o `#[func]` `on_step_pressed` de
/// `EnvironmentClock`, que devolve o passo para cá; os atalhos e "Aplicar" seguem a mesma regra.
pub struct TimePanel {
    root: Gd<CanvasLayer>,
    /// Um por campo, na ordem de `Field::ALL`.
    steppers: Vec<Stepper>,
    /// Data e hora escolhidas nos campos (ainda não aplicadas ao relógio).
    picked: Picked,
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
        let ui = style::ui_look();
        panel.set_theme(&ui_theme(&ui));

        let mut column = VBoxContainer::new_alloc();
        let title = Self::label(&text("time.title", lang));
        let current = Self::label("");

        // Data (dia, mês, ano) numa linha; hora e minuto na de baixo. A ordem das filhas é a
        // ordem do Tab e das setas para os lados.
        let mut date_row = HBoxContainer::new_alloc();
        let mut time_row = HBoxContainer::new_alloc();
        let mut steppers = Vec::new();
        for (index, field) in Field::ALL.into_iter().enumerate() {
            let (stepper, group) = Self::stepper(field, index, lang);
            match field {
                Field::Hour => {
                    time_row.add_child(&group);
                    let colon = Self::label(":");
                    time_row.add_child(&colon);
                }
                Field::Minute => time_row.add_child(&group),
                _ => date_row.add_child(&group),
            }
            steppers.push(stepper);
        }

        let mut set_button = Button::new_alloc();
        set_button.set_text(&text("time.set", lang));

        let mut speed_row = HBoxContainer::new_alloc();
        let speed_label = Self::label(&text("time.speed_label", lang));
        let mut speed = OptionButton::new_alloc();
        speed.set_h_size_flags(SizeFlags::EXPAND_FILL);
        for option in Speed::ALL {
            speed.add_item(&text(option.key(), lang));
        }
        speed_row.add_child(&speed_label);
        speed_row.add_child(&speed);

        let mut shortcut_grid = GridContainer::new_alloc();
        shortcut_grid.set_columns(3);

        let events = Self::wrapped_label("");
        let mut error = Self::wrapped_label("");
        error.set_visible(false);
        let hint = Self::wrapped_label(&text("time.hint", lang));

        column.add_child(&title);
        column.add_child(&current);
        column.add_child(&date_row);
        column.add_child(&time_row);
        column.add_child(&set_button);
        column.add_child(&speed_row);
        column.add_child(&shortcut_grid);
        column.add_child(&events);
        column.add_child(&error);
        column.add_child(&hint);
        let mut margin = MarginContainer::new_alloc();
        for side in [Side::LEFT, Side::RIGHT, Side::TOP, Side::BOTTOM] {
            margin.add_theme_constant_override(
                &format!("margin_{}", side_name(side)),
                PANEL_PADDING_PX,
            );
        }
        margin.add_child(&column);
        panel.add_child(&margin);
        root.add_child(&panel);

        // Esquerda e direita andam só entre o menos e o mais do mesmo campo, e param nas bordas
        // (Tab é que passa para o próximo campo). Sem isso a seta caía no botão de baixo, "Aplicar".
        // Um caminho para o próprio botão ("." ) faz a seta ficar onde está.
        for (index, stepper) in steppers.iter_mut().enumerate() {
            let plus_path = NodePath::from(format!("../Plus{index}").as_str());
            let minus_path = NodePath::from(format!("../Minus{index}").as_str());
            let stay = NodePath::from(".");
            stepper.minus.set_focus_neighbor(Side::LEFT, &stay);
            stepper.minus.set_focus_neighbor(Side::RIGHT, &plus_path);
            stepper.plus.set_focus_neighbor(Side::LEFT, &minus_path);
            stepper.plus.set_focus_neighbor(Side::RIGHT, &stay);
        }

        // Os sinais chamam os `#[func]` do relógio, com o mesmo nome (ver `environment/clock.rs`).
        let on = |method: &str| Callable::from_object_method(clock, method);
        for (index, stepper) in steppers.iter().enumerate() {
            let index = index as i64;
            let step = on("on_step_pressed");
            stepper
                .minus
                .clone()
                .connect("pressed", &step.bindv(&varray![index, -1i64]));
            stepper
                .plus
                .clone()
                .connect("pressed", &step.bindv(&varray![index, 1i64]));
        }
        set_button.connect("pressed", &on("on_set_pressed"));
        speed.connect("item_selected", &on("on_speed_selected"));
        for (key, method) in [
            ("time.now", "on_now_pressed"),
            ("time.sunrise", "on_sunrise_pressed"),
            ("time.sunset", "on_sunset_pressed"),
            ("time.midnight", "on_midnight_pressed"),
            ("time.full_moon", "on_full_moon_pressed"),
            ("time.new_moon", "on_new_moon_pressed"),
        ] {
            let mut button = Button::new_alloc();
            button.set_text(&text(key, lang));
            button.set_h_size_flags(SizeFlags::EXPAND_FILL);
            button.connect("pressed", &on(method));
            shortcut_grid.add_child(&button);
        }

        TimePanel {
            root,
            steppers,
            // Substituído por `show_fields` ao abrir o painel.
            picked: Picked::from_unix(0.0),
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

    /// Seletor de velocidade com o menu aberto: Esc então fecha só o menu.
    pub fn popup_open(&self) -> bool {
        self.speed
            .get_popup()
            .is_some_and(|popup| popup.is_visible())
    }

    /// Mostra ou esconde. Ao abrir, o foco vai para o primeiro campo; ao fechar, solta o foco
    /// para o teclado voltar ao carro.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        self.root.set_visible(open);
        if open {
            if let Some(first) = self.steppers.first_mut() {
                first.minus.grab_focus();
            }
        } else {
            // Any control of the panel may hold the focus (a stepper, the speed selector or a
            // shortcut), so release whichever one it is, not only the steppers.
            if let Some(viewport) = self.root.get_viewport() {
                if let Some(owner) = viewport.gui_get_focus_owner() {
                    if self.root.is_ancestor_of(&owner.clone().upcast::<Node>()) {
                        owner.clone().release_focus();
                    }
                }
            }
        }
    }

    /// Data e hora escolhidas nos campos, para aplicar.
    pub fn picked(&self) -> Picked {
        self.picked
    }

    /// Põe os campos na data e hora locais de `unix` e esconde o erro.
    pub fn show_fields(&mut self, unix: f64) {
        self.picked = Picked::from_unix(unix);
        self.show_picked();
        self.show_error(false);
    }

    /// Anda um campo `delta` passos (botões de menos e de mais, ou o `clock` pelas setas).
    pub fn step(&mut self, field: Field, delta: i64) {
        self.picked = self.picked.step(field, delta);
        self.show_picked();
    }

    /// Passo pelas setas quando o foco está num botão de campo. Devolve `true` se o foco estava
    /// num campo e o passo foi dado (o evento então não segue para o foco).
    pub fn step_focused(&mut self, focus: Option<InstanceId>, delta: i64) -> bool {
        let Some(focus) = focus else {
            return false;
        };
        let Some(field) = self
            .steppers
            .iter()
            .find(|s| s.minus.instance_id() == focus || s.plus.instance_id() == focus)
            .map(|s| s.field)
        else {
            return false;
        };
        self.step(field, delta);
        true
    }

    /// Mostra ou esconde a mensagem de hora inválida (atalho sem alvo, ex.: sem nascer do sol).
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

    /// Escreve o valor de cada campo a partir de `picked`.
    fn show_picked(&mut self) {
        for stepper in self.steppers.iter_mut() {
            let value = self.picked.value(stepper.field);
            let shown = if stepper.field == Field::Year {
                format!("{value:04}")
            } else {
                format!("{value:02}")
            };
            stepper.value.set_text(&shown);
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

    /// Um campo: rótulo em cima, e embaixo "−  valor  +". Devolve também o grupo para a linha.
    fn stepper(field: Field, index: usize, lang: Lang) -> (Stepper, Gd<VBoxContainer>) {
        let name = text(field.key(), lang);
        let mut group = VBoxContainer::new_alloc();
        group.set_h_size_flags(SizeFlags::EXPAND_FILL);
        let caption = Self::label(&name);

        let mut row = HBoxContainer::new_alloc();
        let mut minus = Button::new_alloc();
        minus.set_name(&format!("Minus{index}"));
        minus.set_text("-");
        minus.set_tooltip_text(&fmt("time.decrease", lang, &[("field", &name)]));
        let mut value = Self::label("");
        value.set_horizontal_alignment(HorizontalAlignment::CENTER);
        value.set_h_size_flags(SizeFlags::EXPAND_FILL);
        value.set_custom_minimum_size(Vector2::new(VALUE_WIDTH_PX, 0.0));
        let mut plus = Button::new_alloc();
        plus.set_name(&format!("Plus{index}"));
        plus.set_text("+");
        plus.set_tooltip_text(&fmt("time.increase", lang, &[("field", &name)]));
        row.add_child(&minus);
        row.add_child(&value);
        row.add_child(&plus);

        group.add_child(&caption);
        group.add_child(&row);
        (
            Stepper {
                field,
                minus,
                plus,
                value,
            },
            group,
        )
    }

    /// Rótulo de uma linha, com a largura do próprio texto.
    fn label(text: &str) -> Gd<Label> {
        let mut label = Label::new_alloc();
        label.set_text(text);
        label
    }

    /// Texto longo que quebra linha, com largura mínima para não encolher a zero.
    fn wrapped_label(text: &str) -> Gd<Label> {
        let mut label = Self::label(text);
        label.set_autowrap_mode(AutowrapMode::WORD_SMART);
        label.set_custom_minimum_size(Vector2::new(CONTENT_WIDTH_PX, 0.0));
        label
    }
}

/// Theme color as a Godot color.
fn rgb(c: style::Rgb) -> Color {
    Color::from_rgb(c[0], c[1], c[2])
}

/// The same color with another alpha (transparency only, so it still comes from the theme).
fn with_alpha(c: Color, alpha: f32) -> Color {
    Color::from_rgba(c.r, c.g, c.b, alpha)
}

/// Rounded box with the theme border and corner radius; `padding` is layout, not a theme field.
fn ui_box(
    ui: &style::UiLook,
    bg: Color,
    line: Color,
    border: i32,
    padding: f32,
) -> Gd<StyleBoxFlat> {
    let mut boxed = StyleBoxFlat::new_gd();
    boxed.set_bg_color(bg);
    boxed.set_border_color(line);
    boxed.set_border_width_all(border);
    boxed.set_corner_radius_all(ui.corner_radius);
    boxed.set_content_margin_all(padding);
    boxed
}

/// Theme of the time panel: ink text, panel surfaces, accent hover and an ink focus ring (the
/// accent is too faint on the panel for a focus indicator, so the ring uses the text color), and
/// a distinct disabled state.
fn ui_theme(ui: &style::UiLook) -> Gd<Theme> {
    let panel = rgb(ui.panel);
    let ink = rgb(ui.ink);
    let accent = rgb(ui.accent);
    let border = rgb(ui.border);
    let width = ui.border_width;
    let focus_width = width.max(FOCUS_RING_PX);
    let mut theme = Theme::new_gd();
    theme.set_color("font_color", "Label", ink);

    // OptionButton is a Button subclass but has its own theme type; style both.
    for kind in ["Button", "OptionButton"] {
        theme.set_color("font_color", kind, ink);
        theme.set_color("font_hover_color", kind, ink);
        theme.set_color("font_focus_color", kind, ink);
        theme.set_color("font_pressed_color", kind, ink);
        theme.set_color("font_disabled_color", kind, with_alpha(ink, 0.55));
        theme.set_color("icon_normal_color", kind, ink);
        theme.set_color("icon_hover_color", kind, ink);
        theme.set_color("icon_focus_color", kind, ink);
        theme.set_color("icon_pressed_color", kind, ink);
        theme.set_color("icon_disabled_color", kind, with_alpha(ink, 0.55));
        theme.set_stylebox(
            "normal",
            kind,
            &ui_box(ui, panel, border, width, BUTTON_PADDING_PX),
        );
        theme.set_stylebox(
            "hover",
            kind,
            &ui_box(
                ui,
                with_alpha(accent, 0.16),
                accent,
                width,
                BUTTON_PADDING_PX,
            ),
        );
        theme.set_stylebox(
            "pressed",
            kind,
            &ui_box(
                ui,
                with_alpha(accent, 0.28),
                accent,
                width,
                BUTTON_PADDING_PX,
            ),
        );
        theme.set_stylebox(
            "focus",
            kind,
            &ui_box(
                ui,
                with_alpha(panel, 0.0),
                ink,
                focus_width,
                BUTTON_PADDING_PX,
            ),
        );
        theme.set_stylebox(
            "disabled",
            kind,
            &ui_box(
                ui,
                with_alpha(panel, 0.55),
                with_alpha(border, 0.45),
                width,
                BUTTON_PADDING_PX,
            ),
        );
    }

    for kind in ["Panel", "PanelContainer"] {
        theme.set_stylebox("panel", kind, &ui_box(ui, panel, border, width, 0.0));
    }

    // The speed selector opens a PopupMenu; it must not fall back to the engine's white theme.
    theme.set_color("font_color", "PopupMenu", ink);
    theme.set_color("font_hover_color", "PopupMenu", ink);
    theme.set_color("font_disabled_color", "PopupMenu", with_alpha(ink, 0.55));
    theme.set_stylebox("panel", "PopupMenu", &ui_box(ui, panel, border, width, 0.0));
    theme.set_stylebox(
        "hover",
        "PopupMenu",
        &ui_box(ui, with_alpha(accent, 0.22), accent, width, 0.0),
    );
    theme
}

/// Nome da borda no tema (`margin_left` etc.).
fn side_name(side: Side) -> &'static str {
    match side {
        Side::LEFT => "left",
        Side::RIGHT => "right",
        Side::TOP => "top",
        _ => "bottom",
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

#[cfg(test)]
mod tests {
    use crate::style::{builtin, Rgb, Style};

    /// WCAG relative luminance of one sRGB channel.
    fn linear(channel: f32) -> f32 {
        let channel = channel.clamp(0.0, 1.0);
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    /// WCAG contrast ratio between two sRGB colors (1.0..=21.0).
    fn contrast(a: Rgb, b: Rgb) -> f32 {
        let luminance =
            |c: Rgb| 0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2]);
        let (la, lb) = (luminance(a), luminance(b));
        let (high, low) = (la.max(lb), la.min(lb));
        (high + 0.05) / (low + 0.05)
    }

    #[test]
    fn focus_ring_stays_readable_on_the_panel() {
        for (id, ui) in [
            ("lowpoly", Style::default().ui),
            ("realistic", builtin("realistic").ui),
        ] {
            // The focus ring is the ink color (see `ui_theme`): WCAG 1.4.11 asks 3:1 for the
            // boundary of a focused control. The accent does not reach it on the panel.
            let ring = contrast(ui.ink, ui.panel);
            assert!(
                ring >= 3.0,
                "{id}: focus ring on panel is {ring:.2}:1, below 3:1"
            );
        }
    }
}
