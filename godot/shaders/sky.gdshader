shader_type sky;

// Céu procedural do ambiente. Os parâmetros vêm de `environment::sky::sky_look` (Rust), por
// quadro; a conta do gradiente espelha `sky_color` (Rust), usada pela cor da neblina.
// Coordenadas do Godot: x leste, y para cima, -z norte.

uniform vec3 zenith = vec3(0.2, 0.4, 0.9);
uniform vec3 mid = vec3(0.5, 0.7, 0.9);
uniform vec3 horizon = vec3(0.7, 0.8, 0.95);
uniform vec3 glow = vec3(1.0, 0.4, 0.12);
uniform vec3 glow_hi = vec3(1.0, 0.85, 0.45);
uniform float glow_strength = 0.0;
uniform float halo_strength = 0.0;

uniform vec3 sun_dir = vec3(0.0, 1.0, 0.0);
uniform vec3 sun_color = vec3(1.0, 0.97, 0.92);
uniform float sun_disk = 1.0;

uniform float star_strength = 0.0;

uniform vec3 moon_dir = vec3(0.0, -1.0, 0.0);
uniform float moon_phase = 0.0;
uniform float moon_visibility = 0.0;

// Ponto de extensão para nuvens: 0 = céu limpo; 1 aplica um véu cinza (ainda sem nuvens).
uniform float cloud_cover = 0.0;

// Raio angular do disco do sol e da lua (~0,27 grau, 0,0047 rad) em 1 - cos(θ), ~θ²/2.
const float DISK_R2 = 1.08e-5;
// Fração do laranja do ocaso que aparece fora do lado do sol (mesma constante em sky.rs).
const float WARM_HAZE = 0.35;

float hash13(vec3 p) {
	return fract(sin(dot(p, vec3(12.9898, 78.233, 37.719))) * 43758.5453);
}

// Cor do céu sem disco/estrelas/lua. Mesma ordem de contas de `sky_color` em sky.rs.
vec3 sky_base(vec3 e) {
	float h = max(e.y, 0.0);
	vec3 c = mix(horizon, mid, smoothstep(0.0, 0.25, h));
	c = mix(c, zenith, smoothstep(0.1, 0.9, h));

	vec2 sun_h = sun_dir.xz / max(length(sun_dir.xz), 1e-4);
	vec2 eye_h = e.xz / max(length(e.xz), 1e-4);
	float side = clamp(dot(sun_h, eye_h) * 1.25 - 0.25, 0.0, 1.0);
	float low = 1.0 - smoothstep(-0.05, 0.45, e.y);
	float band = smoothstep(0.0, 0.08, e.y) * (1.0 - smoothstep(0.1, 0.35, e.y));
	c = mix(c, glow, glow_strength * side * low);
	c = mix(c, glow_hi, glow_strength * side * band * 0.8);
	// Calor suave também fora do lado do sol, perto do horizonte (zero do lado oposto).
	float wide = clamp(dot(sun_h, eye_h) * 0.5 + 0.5, 0.0, 1.0);
	c = mix(c, glow, glow_strength * wide * low * WARM_HAZE);

	float cs = max(dot(e, sun_dir), 0.0);
	c += glow_hi * halo_strength * pow(cs, 6.0) * 0.5;
	return c;
}

void sky() {
	vec3 e = normalize(EYEDIR);
	vec3 col = sky_base(e);

	// Disco do sol: ganho alto para ficar branco-quente no alto e laranja no ocaso.
	float d_sun = 1.0 - dot(e, sun_dir);
	float disk = (1.0 - smoothstep(0.5 * DISK_R2, 1.5 * DISK_R2, d_sun)) * sun_disk;
	col = mix(col, sun_color * 8.0, disk);

	// Disco da lua com fase por máscara: a parte iluminada fica do lado do sol. A fração
	// iluminada k define o terminador: lit onde dot(uv, ld) > 1 - 2k.
	float d_moon = 1.0 - dot(e, moon_dir);
	float moon_disk = (1.0 - smoothstep(0.8 * DISK_R2, 1.2 * DISK_R2, d_moon)) * moon_visibility;
	if (moon_disk > 0.0) {
		vec3 up = abs(moon_dir.y) < 0.99 ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0);
		vec3 mt = normalize(cross(up, moon_dir));
		vec3 mb = cross(moon_dir, mt);
		vec2 uv = vec2(dot(e - moon_dir, mt), dot(e - moon_dir, mb)) / sqrt(DISK_R2 * 2.0);
		vec2 ld = vec2(dot(sun_dir, mt), dot(sun_dir, mb));
		ld /= max(length(ld), 1e-4);
		float term = 1.0 - 2.0 * moon_phase;
		float lit = smoothstep(term - 0.15, term + 0.15, dot(uv, ld));
		vec3 moon_col = mix(vec3(0.03, 0.035, 0.05), vec3(0.92, 0.93, 0.88), lit);
		col = mix(col, moon_col, moon_disk);
	}

	// Estrelas: grade na direção, uma por célula com probabilidade baixa; só acima do horizonte.
	if (star_strength > 0.0 && e.y > 0.0) {
		vec3 p = e * 150.0;
		vec3 id = floor(p);
		vec3 f = fract(p) - 0.5;
		float h = hash13(id);
		float star = step(0.993, h) * (1.0 - smoothstep(0.1, 0.35, length(f)));
		float twinkle = 0.8 + 0.2 * sin(TIME * 2.0 + h * 60.0);
		col += vec3(0.85, 0.9, 1.0) * star * twinkle * star_strength * smoothstep(0.0, 0.2, e.y) * 0.8;
	}

	col = mix(col, vec3(dot(col, vec3(0.333))), cloud_cover * 0.5);
	COLOR = col;
}
