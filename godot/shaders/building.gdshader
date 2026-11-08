shader_type spatial;

// Buildings (B1 + L1c): the vertex color is the facade. Windows are a mask on the wall, no geometry.
// UV (from building.rs): x = perimeter in m (continuous around the ring), y = height above the flat
// base in m. A roof has x < 0. UV2.x = seed of the building, 0..1. UV2.y = height of the vertex above
// the terrain under it, in m: windows, doors and the shop band only show above 1 m of ground.
// Day: a 1.5 m x 1.2 m window (sill 1.0 m, 3 m pitch, 3 m floors) is a bluish-grey glass. Ground floor:
// half of the buildings have a dark shop band instead. Night: a share of the windows of each building
// (30-60%, by seed) is lit, warm or cold by seed, after a delay of 0..25 min after the street lamps.
// Windows fade to the plain facade past ~200-260 m (no moire). No textures.
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
	ROUGHNESS = 0.6;

	// Night: the building lights up after its delay, and only some of its windows.
	float since = lamp_now - lamp_on_time - seed * delay_max_s;
	float on_global = lamp_state > 0.5 ? smoothstep(0.0, ramp_s, since) : 0.0;
	float share = mix(lit_min, lit_max, fract(seed * 7.31));
	float lit = step(hash(vec2(floor(u / window_pitch), floor_i) + seed * 101.0), share);
	vec3 tint = fract(seed * 13.7) < 0.5 ? color_warm : color_cold;
	EMISSION = tint * on_global * emission_gain * (win * lit + band * 0.3);
}
