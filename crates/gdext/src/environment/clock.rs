//! Ambiente da cena: relógio do mundo, sol e lua como luzes, céu, neblina e tonemapping.
//! A astronomia e os limiares vivem em `sky.rs` (puros); aqui fica só a ponte com Godot.

use std::time::{SystemTime, UNIX_EPOCH};

use godot::classes::directional_light_3d::ShadowMode;
use godot::classes::environment::{AmbientSource, BgMode, ToneMapper};
use godot::classes::light_3d::Param;
use godot::classes::rendering_server::GlobalShaderParameterType;
use godot::classes::RenderingServer;
use godot::classes::{
    DirectionalLight3D, Environment, INode, InputEvent, Node, ProceduralSkyMaterial, Sky,
    WorldEnvironment,
};
use godot::prelude::*;

use super::sky::{self, Crossing, LightThreshold, ThresholdBank};

/// Passo de tempo do atalho de depuração `[` e `]`, em segundos de jogo.
const DEBUG_STEP_S: f64 = 15.0 * 60.0;
/// Velocidade do relógio ao alternar com `\` (ou no padrão).
const FAST_SPEED: f64 = 60.0;
/// Período do log de hora e luz, em segundos reais.
const LOG_PERIOD_S: f64 = 5.0;
/// Energia da luz direta do sol ao meio-dia (Godot usa ~1 como padrão).
const SUN_MAX_ENERGY: f64 = 1.6;
/// Energia da luz da lua na fase cheia e alta.
const MOON_MAX_ENERGY: f64 = 0.25;
/// Cor da luz da lua, azulada.
const MOON_COLOR: (f32, f32, f32) = (0.6, 0.7, 1.0);
/// Distância de sombra do sol a partir da câmera, em metros.
const SHADOW_MAX_DISTANCE_M: f32 = 300.0;
/// Ambiente mínimo com o sol embaixo do horizonte, fração da energia ambiente.
const AMBIENT_MIN: f64 = 0.3;
/// Multiplicador do clima (futuro); 1 = céu limpo.
const WEATHER_FACTOR: f64 = 1.0;
/// Limiar de luz dos postes: acendem ao cair abaixo de `LAMP_ON_BELOW`, apagam acima de `LAMP_OFF_ABOVE`.
const LAMP_ON_BELOW: f64 = 0.40;
const LAMP_OFF_ABOVE: f64 = 0.50;
/// Globais de shader dos postes: instante do acendimento (s de jogo desde o início, -1 apagado),
/// estado (1 aceso, 0 apagado) e o instante de jogo atual (s desde o início).
const LAMP_GLOBALS: [&str; 3] = ["lamp_on_time", "lamp_state", "lamp_now"];
/// Cor do chão embaixo do horizonte do céu.
const GROUND_BOTTOM: Color = Color::from_rgb(0.12, 0.13, 0.16);

/// Relógio do mundo e luz ambiente. Criado por `EarthWorld` com o nome "Environment".
#[derive(GodotClass)]
#[class(base = Node, init)]
pub struct EnvironmentClock {
    base: Base<Node>,
    /// Data inicial, dia civil local (BRT).
    #[init(val = 2026)]
    #[export]
    start_year: i64,
    #[init(val = 11)]
    #[export]
    start_month: i64,
    #[init(val = 15)]
    #[export]
    start_day: i64,
    /// Começa `start_minutes_before_sunset` antes do ocaso do dia inicial; senão, na hora abaixo.
    #[init(val = true)]
    #[export]
    start_at_sunset: bool,
    #[init(val = 30.0)]
    #[export]
    start_minutes_before_sunset: f64,
    #[init(val = 17.0)]
    #[export]
    start_hour_local: f64,
    /// Segundos de jogo por segundo real. 1 = tempo real; 0 = fixo.
    #[init(val = FAST_SPEED)]
    #[export]
    speed: f64,
    /// Segue a hora do sistema, em vez de avançar com `speed`.
    #[init(val = false)]
    #[export]
    real_time: bool,
    /// Sombras do sol em cascatas (2 splits) até `SHADOW_MAX_DISTANCE_M`.
    #[init(val = true)]
    #[export]
    shadows_enabled: bool,

    /// Instante do mundo, segundos Unix (UTC).
    sim_unix: f64,
    /// Luz global 0..1 do último quadro.
    light: f64,
    sun_alt_deg: f64,
    world_env: Option<Gd<WorldEnvironment>>,
    /// Sol antigo da cena, liberado ao criar o novo.
    replaced_sun: Option<Gd<Node>>,
    env: Option<Gd<Environment>>,
    sky_mat: Option<Gd<ProceduralSkyMaterial>>,
    sun: Option<Gd<DirectionalLight3D>>,
    moon: Option<Gd<DirectionalLight3D>>,
    bank: ThresholdBank,
    listeners: Vec<Box<dyn FnMut(&Crossing)>>,
    log_timer: f64,
    /// `sim_unix` at the start: game time in the lamp globals counts from here.
    origin_unix: f64,
    /// Game seconds since the start when the street lamps came on; `None` while they are off.
    lamp_on_since: Option<f64>,
    lamps_primed: bool,
}

#[godot_api]
impl EnvironmentClock {
    /// Emitted when the light crosses a threshold: `descending` true when it fell to `on_below`,
    /// false when it rose above `off_above`.
    #[signal]
    fn light_threshold_crossed(name: GString, descending: bool);
}

impl EnvironmentClock {
    /// Liga o relógio à cena antes de entrar na árvore: `WorldEnvironment` a ter o ambiente
    /// e o sol antigo a ser removido. Chamar antes de `add_child`.
    pub fn attach(&mut self, world_env: Option<Gd<WorldEnvironment>>, old_sun: Option<Gd<Node>>) {
        self.world_env = world_env;
        self.replaced_sun = old_sun;
    }

    /// Registra um limiar de luz (postes, janelas, faróis). Sinal e listeners são emitidos
    /// só no cruzamento.
    #[allow(dead_code)] // API for consumers (lamps, windows, headlights) in later slices
    pub fn add_threshold(&mut self, threshold: LightThreshold) {
        self.bank.add(threshold);
    }

    /// Assina os cruzamentos de limiar a partir de Rust (ex.: postes em outro módulo).
    #[allow(dead_code)] // API for consumers in later slices
    pub fn subscribe(&mut self, listener: impl FnMut(&Crossing) + 'static) {
        self.listeners.push(Box::new(listener));
    }

    /// Luz global do último quadro, 0..1.
    #[allow(dead_code)] // API for consumers in later slices
    pub fn light(&self) -> f64 {
        self.light
    }

    /// Game seconds since the start, and when the street lamps came on (if they are on).
    pub fn lamp_clock(&self) -> (f64, Option<f64>) {
        (self.sim_unix - self.origin_unix, self.lamp_on_since)
    }

    /// Pushes the lamp state to the shader globals.
    fn push_lamp_state(&self) {
        let (on_time, state) = match self.lamp_on_since {
            Some(t) => (t as f32, 1.0f32),
            None => (-1.0f32, 0.0f32),
        };
        set_global(LAMP_GLOBALS[0], on_time);
        set_global(LAMP_GLOBALS[1], state);
    }

    fn start_unix(&self) -> f64 {
        let midnight = sky::local_to_unix(
            self.start_year,
            self.start_month as u32,
            self.start_day as u32,
            0.0,
        );
        if self.start_at_sunset {
            if let Some(sunset) = sky::next_sunset(midnight) {
                return sunset - 60.0 * self.start_minutes_before_sunset;
            }
            godot_warn!("environment: no sunset found on the start day, using start_hour_local");
        }
        sky::local_to_unix(
            self.start_year,
            self.start_month as u32,
            self.start_day as u32,
            self.start_hour_local,
        )
    }

    fn system_unix() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    fn setup_lights(&mut self) {
        let mut sun = DirectionalLight3D::new_alloc();
        sun.set_shadow(self.shadows_enabled);
        sun.set_shadow_mode(ShadowMode::PARALLEL_2_SPLITS);
        sun.set_param(Param::SHADOW_MAX_DISTANCE, SHADOW_MAX_DISTANCE_M);
        sun.set_name("Sun");
        self.base_mut().add_child(&sun);
        self.sun = Some(sun);

        let mut moon = DirectionalLight3D::new_alloc();
        moon.set_shadow(false);
        moon.set_color(Color::from_rgb(MOON_COLOR.0, MOON_COLOR.1, MOON_COLOR.2));
        moon.set_name("Moon");
        self.base_mut().add_child(&moon);
        self.moon = Some(moon);
    }

    fn setup_environment(&mut self) {
        let Some(world_env) = self.world_env.as_mut() else {
            godot_warn!("environment: no WorldEnvironment under the scene root, sky not driven");
            return;
        };
        let mut sky_mat = ProceduralSkyMaterial::new_gd();
        let mut sky = Sky::new_gd();
        sky.set_material(&sky_mat);
        let mut env = Environment::new_gd();
        env.set_background(BgMode::SKY);
        env.set_sky(&sky);
        env.set_ambient_source(AmbientSource::SKY);
        env.set_tonemapper(ToneMapper::AGX);
        sky_mat.set_ground_bottom_color(GROUND_BOTTOM);
        world_env.set_environment(&env);
        self.env = Some(env);
        self.sky_mat = Some(sky_mat);
    }

    /// Orienta a luz direcional: ela aponta do sol para a cena (o -Z local vai para baixo/atrás).
    fn orient(light: &mut Gd<DirectionalLight3D>, to_body: Vector3) {
        // Vetor +Z local aponta para o astro; o -Z (direção da luz) aponta para a cena.
        let z = to_body.normalized();
        let up = Vector3::UP;
        let x = up.cross(z).normalized();
        let y = z.cross(x);
        light.set_basis(Basis::from_cols(x, y, z));
    }

    /// Direção do astro (de onde a luz vem) em coordenadas Godot: x leste, y cima, -z norte.
    fn direction_to(azimuth_deg: f64, altitude_deg: f64) -> Vector3 {
        let az = azimuth_deg.to_radians();
        let alt = altitude_deg.to_radians();
        Vector3::new(
            (az.sin() * alt.cos()) as f32,
            alt.sin() as f32,
            (-az.cos() * alt.cos()) as f32,
        )
    }

    fn apply_visuals(&mut self, sun: sky::Horizontal, moon: sky::MoonState) {
        let alt = sun.altitude_deg;
        if let Some(light) = self.sun.as_mut() {
            Self::orient(light, Self::direction_to(sun.azimuth_deg, alt));
            let tint = sky::sun_tint(alt);
            light.set_color(Color::from_rgb(tint[0], tint[1], tint[2]));
            light.set_param(Param::ENERGY, sky::sun_energy(alt, SUN_MAX_ENERGY) as f32);
        }
        if let Some(light) = self.moon.as_mut() {
            Self::orient(
                light,
                Self::direction_to(moon.position.azimuth_deg, moon.position.altitude_deg),
            );
            let height = (moon.position.altitude_deg / sky::MOON_FULL_ALT_DEG).clamp(0.0, 1.0);
            light.set_param(
                Param::ENERGY,
                (MOON_MAX_ENERGY * moon.illumination * height) as f32,
            );
        }
        if let (Some(mat), Some(env)) = (self.sky_mat.as_mut(), self.env.as_mut()) {
            let horizon = sky::horizon_tint(alt);
            let zenith = sky::zenith_tint(alt);
            let horizon = Color::from_rgb(horizon[0], horizon[1], horizon[2]);
            mat.set_sky_top_color(Color::from_rgb(zenith[0], zenith[1], zenith[2]));
            mat.set_sky_horizon_color(horizon);
            mat.set_ground_horizon_color(horizon);
            // A neblina de profundidade (lod.rs) segue a cor do horizonte do céu.
            env.set_fog_light_color(horizon);
            env.set_ambient_light_energy((AMBIENT_MIN + (1.0 - AMBIENT_MIN) * self.light) as f32);
        }
    }

    fn local_clock(&self) -> String {
        let h = sky::local_hour(self.sim_unix);
        format!(
            "{:02}:{:02}",
            h.floor() as u32,
            ((h.fract()) * 60.0).floor() as u32
        )
    }

    fn change_speed(&mut self, speed: f64) {
        self.speed = speed;
        self.real_time = false;
        godot_print!("environment: speed {speed}x");
    }
}

#[godot_api]
impl INode for EnvironmentClock {
    fn ready(&mut self) {
        self.sim_unix = if self.real_time {
            Self::system_unix()
        } else {
            self.start_unix()
        };
        self.origin_unix = self.sim_unix;
        for name in LAMP_GLOBALS {
            RenderingServer::singleton().global_shader_parameter_add(
                name,
                GlobalShaderParameterType::FLOAT,
                &Variant::from(-1.0f32),
            );
        }
        self.setup_lights();
        self.setup_environment();
        if let Some(old) = self.replaced_sun.take() {
            let mut old = old;
            old.queue_free();
        }
        self.base_mut().set_process(true);
        self.base_mut().set_process_unhandled_input(true);
        // Limiar de postes: acendem em 0,40 e apagam em 0,50 (histerese).
        self.bank.add(LightThreshold {
            name: "street_lamps".into(),
            on_below: LAMP_ON_BELOW,
            off_above: LAMP_OFF_ABOVE,
        });
        godot_print!(
            "environment: start {} (local), speed {}x, real_time {}",
            self.local_clock(),
            self.speed,
            self.real_time
        );
    }

    fn process(&mut self, delta: f64) {
        if self.real_time {
            self.sim_unix = Self::system_unix();
        } else if self.speed > 0.0 {
            self.sim_unix += delta * self.speed;
        }
        let sun = sky::sun(self.sim_unix);
        let moon = sky::moon(self.sim_unix);
        self.sun_alt_deg = sun.altitude_deg;
        self.light = sky::light_level(sun.altitude_deg, &moon, WEATHER_FACTOR);
        self.apply_visuals(sun, moon);

        let now = self.sim_unix - self.origin_unix;
        set_global(LAMP_GLOBALS[2], now as f32);
        // The first sample sets the threshold state without a signal: a clock that starts at night
        // turns the lamps on right away.
        if !self.lamps_primed {
            self.lamps_primed = true;
            if self.light < LAMP_ON_BELOW {
                self.lamp_on_since = Some(now);
            }
            self.push_lamp_state();
        }

        for crossing in self.bank.update(self.light) {
            if crossing.name == "street_lamps" {
                self.lamp_on_since = crossing.descending.then_some(now);
                self.push_lamp_state();
                godot_print!("environment: street lamps {} at {:.0} s", if crossing.descending { "on" } else { "off" }, now);
            }
            godot_print!(
                "environment: light threshold '{}' crossed, descending={} (light {:.3})",
                crossing.name,
                crossing.descending,
                self.light
            );
            let name = GString::from(crossing.name.as_str());
            self.signals()
                .light_threshold_crossed()
                .emit(&name, crossing.descending);
            for listener in self.listeners.iter_mut() {
                listener(&crossing);
            }
        }

        self.log_timer += delta;
        if self.log_timer >= LOG_PERIOD_S {
            self.log_timer = 0.0;
            godot_print!(
                "environment: local {} sun_alt {:.2} deg light {:.3} street_lamps {}",
                self.local_clock(),
                self.sun_alt_deg,
                self.light,
                if self.lamp_on_since.is_some() { "on" } else { "off" }
            );
        }
    }

    fn unhandled_input(&mut self, event: Gd<InputEvent>) {
        if event.is_action_pressed("env_time_back") {
            self.sim_unix -= DEBUG_STEP_S;
        } else if event.is_action_pressed("env_time_forward") {
            self.sim_unix += DEBUG_STEP_S;
        } else if event.is_action_pressed("env_speed_toggle") {
            let next = if self.speed > 0.0 { 0.0 } else { FAST_SPEED };
            self.change_speed(next);
        }
    }
}

/// Sets a float shader global.
fn set_global(name: &str, value: f32) {
    RenderingServer::singleton().global_shader_parameter_set(name, &Variant::from(value));
}
