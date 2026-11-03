shader_type spatial;
render_mode blend_add, unshaded, cull_disabled, depth_draw_never, shadows_disabled;

// Distant lights (L2): one billboard quad per light, sized by its distance so that it covers a
// fixed angle (a few pixels). Depth test stays on (the relief hides them), no shadow, no real light.
// Per instance (MultiMesh custom data): r = delay after the street lamps came on (s), g = hue
// 0..1 (sodium to warm white), b = weight 0..1 (buildings in the cell, slightly bigger).
global uniform float lamp_on_time;
global uniform float lamp_state;
global uniform float lamp_now;

uniform vec3 color_sodium : source_color = vec3(1.0, 0.72, 0.35);
uniform vec3 color_warm : source_color = vec3(1.0, 0.9, 0.75);
// Cor da neblina na direção do olhar, atualizada a cada quadro pelo relógio (environment/clock.rs).
global uniform vec3 haze_color;
uniform float fog_begin = 5000.0;
uniform float fog_end = 10000.0;
uniform float fade_begin = 600.0;
uniform float fade_end = 800.0;
uniform float angular_size = 0.003;
uniform float intensity = 2.5;
uniform float ramp_s = 30.0;

varying vec2 corner;
varying float glow;
varying vec3 tint;

float hash(vec2 p) {
	return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

void vertex() {
	vec3 center = (MODEL_MATRIX * vec4(0.0, 0.0, 0.0, 1.0)).xyz;
	vec3 eye = (INV_VIEW_MATRIX * vec4(0.0, 0.0, 0.0, 1.0)).xyz;
	float d = distance(center, eye);
	float half_size = d * angular_size * (0.8 + 0.6 * INSTANCE_CUSTOM.b);
	vec3 right = INV_VIEW_MATRIX[0].xyz;
	vec3 up = INV_VIEW_MATRIX[1].xyz;
	vec3 wpos = center + (right * VERTEX.x + up * VERTEX.y) * half_size;
	POSITION = PROJECTION_MATRIX * VIEW_MATRIX * vec4(wpos, 1.0);

	corner = VERTEX.xy;
	// Lit once the lamps are on and its own delay has passed, ramped in over `ramp_s`.
	float since = lamp_now - lamp_on_time - INSTANCE_CUSTOM.r;
	float lit = lamp_state > 0.5 ? smoothstep(0.0, ramp_s, since) : 0.0;
	// Cheap per-light flicker: a hash of the position sets the rate and the phase.
	float h = hash(center.xz + vec2(center.y * 0.37, 0.0));
	float flicker = 1.0 + 0.08 * sin(TIME * (2.0 + 3.0 * h) + 6.2831 * h);
	float fade = smoothstep(fade_begin, fade_end, d);
	float fog = smoothstep(fog_begin, fog_end, d);
	glow = lit * flicker * fade * (1.0 - fog);
	// Far lights take the colour of the haze as they fade into it.
	tint = mix(mix(color_sodium, color_warm, INSTANCE_CUSTOM.g), haze_color, fog * 0.5);
}

void fragment() {
	// Round point with a soft edge; alpha folded into the colour so that any blend mode adds it.
	float disc = smoothstep(1.0, 0.0, length(corner));
	ALBEDO = tint * (intensity * glow * disc);
	ALPHA = 1.0;
}
