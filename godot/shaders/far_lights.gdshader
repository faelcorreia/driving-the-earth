shader_type spatial;
render_mode blend_add, unshaded, cull_disabled, depth_draw_never, shadows_disabled;

// Distant lights (L2): one billboard quad per light, sized by its distance so that it covers a
// fixed angle (a few pixels), with a soft floor on its size in pixels. Depth test stays on (the
// relief hides them), but the light is lifted by `lift_m` so that a coarse ring does not cut it off.
// No shadow, no real light. Fades out over a band of `swap_band_m` around `swap_m`, where the real
// street lamp takes over (the handover is continuous in colour and intensity).
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
// Handover to the real street lamp: the light is off nearer than swap_m - swap_band_m / 2 and full
// farther than swap_m + swap_band_m / 2. Mirrors `handover_fade` in world/far_lights.rs.
uniform float swap_m = 600.0;
uniform float swap_band_m = 20.0;
// Height added to the light, in meters (the lamp heads are the reference, the relief is not).
uniform float lift_m = 4.0;
// Soft floor on the radius on screen, in pixels (`soft_min_radius_px` in world/far_lights.rs).
uniform float min_radius_px = 1.25;
uniform float angular_size = 0.003;
uniform float intensity = 2.5;
uniform float ramp_s = 30.0;

varying vec2 corner;
varying float glow;
varying vec3 tint;

void vertex() {
	vec3 center = (MODEL_MATRIX * vec4(0.0, 0.0, 0.0, 1.0)).xyz + vec3(0.0, lift_m, 0.0);
	vec3 eye = (INV_VIEW_MATRIX * vec4(0.0, 0.0, 0.0, 1.0)).xyz;
	float d = distance(center, eye);
	// Radius in meters for the angular size, then in pixels; a sub-pixel light is floored softly.
	float px_per_m = PROJECTION_MATRIX[1][1] * 0.5 * VIEWPORT_SIZE.y / d;
	float radius_px = d * angular_size * (0.8 + 0.6 * INSTANCE_CUSTOM.b) * px_per_m;
	float soft_px = sqrt(radius_px * radius_px + min_radius_px * min_radius_px);
	float half_size = soft_px / px_per_m;
	vec3 right = INV_VIEW_MATRIX[0].xyz;
	vec3 up = INV_VIEW_MATRIX[1].xyz;
	vec3 wpos = center + (right * VERTEX.x + up * VERTEX.y) * half_size;
	POSITION = PROJECTION_MATRIX * VIEW_MATRIX * vec4(wpos, 1.0);

	corner = VERTEX.xy;
	// Lit once the lamps are on and its own delay has passed, ramped in over `ramp_s`.
	float since = lamp_now - lamp_on_time - INSTANCE_CUSTOM.r;
	float lit = lamp_state > 0.5 ? smoothstep(0.0, ramp_s, since) : 0.0;
	float t = clamp((d - (swap_m - swap_band_m * 0.5)) / swap_band_m, 0.0, 1.0);
	float fade = t * t * (3.0 - 2.0 * t);
	float fog = smoothstep(fog_begin, fog_end, d);
	glow = lit * fade * (1.0 - fog);
	// Far lights take the colour of the haze as they fade into it.
	tint = mix(mix(color_sodium, color_warm, INSTANCE_CUSTOM.g), haze_color, fog * 0.5);
}

void fragment() {
	// Round point with a soft edge; alpha folded into the colour so that any blend mode adds it.
	float disc = smoothstep(1.0, 0.0, length(corner));
	ALBEDO = tint * (intensity * glow * disc);
	ALPHA = 1.0;
}
