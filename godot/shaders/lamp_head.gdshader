shader_type spatial;

// Lamp head of a street lamp, drawn with a MultiMesh: each instance lights up at its own delay
// (INSTANCE_CUSTOM.r, seconds after the street-lamp crossing) once the lamp state is on.
global uniform float lamp_on_time;
global uniform float lamp_state;
global uniform float lamp_now;

uniform vec3 lamp_color : source_color = vec3(1.0, 0.72, 0.35);
uniform float lamp_energy = 4.0;

varying float lit;

void vertex() {
	lit = (lamp_state > 0.5 && lamp_now >= lamp_on_time + INSTANCE_CUSTOM.r) ? 1.0 : 0.0;
}

void fragment() {
	ALBEDO = mix(vec3(0.25, 0.25, 0.27), lamp_color, lit);
	EMISSION = lamp_color * lit * lamp_energy;
	ROUGHNESS = 0.4;
}
