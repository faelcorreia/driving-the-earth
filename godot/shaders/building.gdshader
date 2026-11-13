shader_type spatial;
render_mode diffuse_lambert, specular_disabled;

// Buildings (B1 + L1c): the vertex color is the facade. Windows are a mask on the wall, no geometry.
// UV (from building.rs): x = perimeter in m (continuous around the ring), y = height above the flat
// base in m. A roof has x < 0. UV2.x = seed of the building, 0..1. UV2.y = height of the vertex above
// the terrain under it, in m: windows, doors and the shop band only show above 1 m of ground.
// Day: a 1.5 m x 1.2 m window (sill 1.0 m, 3 m pitch, 3 m floors) is a bluish-grey glass. Ground floor:
// half of the buildings have a dark shop band instead. Night: a share of the windows of each building
// (30-60%, by seed) is lit, warm or cold by seed, after a delay of 0..25 min after the street lamps.
// Windows fade to the plain facade past ~200-260 m (no moire). The facade is matte (no metallic,
// roughness 1, no direct specular in the toon light) so the low-poly facets read as flat paint.
// No textures.
// Light (backlog 125): the same soft two-band toon light as `toon_world.gdshader`, with the same
// warm/lilac bands and the discreet rim on the sun side. It is only in `light()`, so the window,
// door and shop masks and the night emission are untouched. `toon_light` false keeps the previous
// plain Lambert shading (the realistic theme), so the mask keeps working without the bands.
global uniform float lamp_on_time;
global uniform float lamp_state;
global uniform float lamp_now;

uniform vec3 glass_color : source_color = vec3(0.20, 0.27, 0.33);
uniform float glass_mix = 0.8;
uniform vec3 color_warm : source_color = vec3(1.0, 0.78, 0.45);
uniform vec3 color_cold : source_color = vec3(0.85, 0.92, 1.0);
uniform float emission_gain = 1.6;
uniform float window_pitch = 3.0;
uniform float floor_h = 3.0;
uniform float fade_begin = 200.0;
uniform float fade_end = 260.0;
uniform float lit_min = 0.3;
uniform float lit_max = 0.6;
uniform float delay_max_s = 1500.0;
uniform float ramp_s = 60.0;
uniform bool toon_light = true;
uniform float band_softness = 0.16;
// Multipliers of the direct light, not colors: warm on the sun side, lilac in the shade band.
uniform vec3 lit_tint = vec3(1.05, 1.0, 0.94);
uniform vec3 shade_tint = vec3(0.92, 0.88, 0.97);
uniform vec3 rim_color : source_color = vec3(1.0, 0.78, 0.55);
uniform float rim_strength = 0.16;

varying vec2 wall_uv;
varying float seed;
varying float dist;
varying float clearance;

float hash(vec2 p) {
	return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

void vertex() {
	wall_uv = UV;
	seed = UV2.x;
	clearance = UV2.y;
	dist = length((MODELVIEW_MATRIX * vec4(VERTEX, 1.0)).xyz);
}

void fragment() {
	float wall = step(0.0, wall_uv.x);
	// Only the wall above 1 m of ground is a facade: the part sunk in the terrain gets no mask.
	float above_ground = step(1.0, clearance);
	float fade = 1.0 - smoothstep(fade_begin, fade_end, dist);

	// Along the wall, the window grid moves by a seed offset. The cell coordinate wraps, but the
	// antialiasing width comes from the continuous coordinate, so the wrap leaves no seam.
	float u = wall_uv.x + seed * window_pitch * 7.0;
	float v = wall_uv.y;
	float floor_i = floor(v / floor_h);
	float cx = fract(u / window_pitch) * window_pitch;
	float cy = v - floor_i * floor_h;
	float ax = max(fwidth(u), 1e-4);
	float ay = max(fwidth(v), 1e-4);
	// Window: 1.5 m wide centered in the 3 m pitch (x 0.75..2.25), 1.2 m high from 1.0 m above the floor.
	float mx = 1.0 - smoothstep(-ax, ax, abs(cx - 1.5) - 0.75);
	float my = 1.0 - smoothstep(-ay, ay, abs(cy - 1.6) - 0.6);

	// Ground floor shop band (no windows there) on about half of the buildings.
	float ground = 1.0 - step(1.0, floor_i);
	float shop = step(seed, 0.5) * ground * wall;
	float band = shop * (1.0 - smoothstep(-ay, ay, cy - 2.2)) * fade * above_ground;
	float win = mx * my * fade * wall * (1.0 - shop) * above_ground;

	vec3 facade = COLOR.rgb;
	facade = mix(facade, glass_color, win * glass_mix);
	facade = mix(facade, COLOR.rgb * 0.45, band * 0.9);
	ALBEDO = facade;
	METALLIC = 0.0;
	SPECULAR = toon_light ? 0.0 : 0.15;
	ROUGHNESS = 1.0;

	// Night: the building lights up after its delay, and only some of its windows.
	float since = lamp_now - lamp_on_time - seed * delay_max_s;
	float on_global = lamp_state > 0.5 ? smoothstep(0.0, ramp_s, since) : 0.0;
	float share = mix(lit_min, lit_max, fract(seed * 7.31));
	float lit = step(hash(vec2(floor(u / window_pitch), floor_i) + seed * 101.0), share);
	vec3 tint = fract(seed * 13.7) < 0.5 ? color_warm : color_cold;
	EMISSION = tint * on_global * emission_gain * (win * lit + band * 0.3);
}

void light() {
	if (!toon_light) {
		// Realistic theme: the plain Lambert term of the previous shader (no bands, no rim).
		DIFFUSE_LIGHT += clamp(dot(NORMAL, LIGHT), 0.0, 1.0) * ATTENUATION * LIGHT_COLOR / PI;
	} else {
		// Soft two bands of direct light, as in `toon_world.gdshader` (same constants, so a wall
		// and the road beside it share the band edges).
		float ndl = dot(NORMAL, LIGHT);
		float wrap = clamp((ndl + 0.35) / 1.35, 0.0, 1.0);
		float low = smoothstep(0.30 - band_softness, 0.30 + band_softness, wrap);
		float high = smoothstep(0.72 - band_softness, 0.72 + band_softness, wrap);
		float bands = low * 0.62 + high * 0.38;
		if (!LIGHT_IS_DIRECTIONAL) {
			// A local light (street lamp) keeps its own color, with one soft edge and no rim; see
			// `toon_world.gdshader` for why its band is near flat.
			float local_bands = low * 0.98 + high * 0.02;
			DIFFUSE_LIGHT += local_bands * ATTENUATION * LIGHT_COLOR / PI;
		} else {
			// How warm the light is: 1 for the low sun, 0 for the cool moon, in between for the
			// high sun.
			float warmth = smoothstep(0.0, 0.25, LIGHT_COLOR.r - LIGHT_COLOR.b);
			// The warm/lilac tint belongs to the sun: the moon keeps its own (cool) color.
			vec3 tint = mix(vec3(1.0), mix(shade_tint, lit_tint, bands), warmth);
			// The shade band keeps a small share of the warm light, tinted lilac, so the side
			// away from the sun reads as painted shade instead of only ambient. Off for cool lights.
			vec3 shade = tint * max(bands, warmth * 0.10);
			DIFFUSE_LIGHT += shade * ATTENUATION * LIGHT_COLOR / PI;

			float fresnel = pow(1.0 - clamp(dot(NORMAL, VIEW), 0.0, 1.0), 3.0);
			float sunward = smoothstep(0.0, 0.35, ndl);
			float rim = rim_strength * warmth * fresnel * sunward;
			DIFFUSE_LIGHT += rim_color * rim * ATTENUATION * LIGHT_COLOR / PI;
		}
	}
}
