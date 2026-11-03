shader_type spatial;
render_mode blend_add, unshaded, cull_disabled, depth_draw_never, shadows_disabled;

// Light pool on the ground under a lit street lamp. Additive, no shadow, no collision. The fan
// mesh carries the radial falloff in its vertex alpha (COLOR.a).
global uniform float lamp_on_time;
global uniform float lamp_state;
global uniform float lamp_now;

uniform vec3 lamp_color : source_color = vec3(1.0, 0.72, 0.35);
uniform float glow_strength = 0.35;

varying float lit;

void vertex() {
	lit = (lamp_state > 0.5 && lamp_now >= lamp_on_time + INSTANCE_CUSTOM.r) ? 1.0 : 0.0;
}

void fragment() {
	ALBEDO = lamp_color * glow_strength * COLOR.a * lit;
}
