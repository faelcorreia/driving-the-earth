//! Labels of the intro scene: the game's name and the skip hint, in the system language (en or
//! pt-BR).

use godot::classes::{ILabel, Label, Os};
use godot::prelude::*;

use crate::ui_text;

/// Label that shows the game's title, translated through `ui_text`.
#[derive(GodotClass)]
#[class(base = Label, init)]
pub struct IntroTitle {
    base: Base<Label>,
}

#[godot_api]
impl ILabel for IntroTitle {
    fn ready(&mut self) {
        let lang = ui_text::language(&Os::singleton().get_locale().to_string());
        self.base_mut().set_text(ui_text::text("intro.title", lang));
    }
}

/// Label that shows how to skip the intro, translated through `ui_text`.
#[derive(GodotClass)]
#[class(base = Label, init)]
pub struct IntroHint {
    base: Base<Label>,
}

#[godot_api]
impl ILabel for IntroHint {
    fn ready(&mut self) {
        let lang = ui_text::language(&Os::singleton().get_locale().to_string());
        self.base_mut().set_text(ui_text::text("intro.hint", lang));
    }
}
