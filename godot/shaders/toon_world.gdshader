shader_type spatial;
render_mode diffuse_lambert, specular_disabled;

// Soft two-band toon light of the low-poly world (backlog 125), shared by the terrain, the roads,
// the buildings and the car, so the same band edges cross a wall, a roof and the road.
//
// Written after the reference in the `com amor` project (`shaders/toon.gdshader`), adapted to this
// game: a custom `light()` replaces the built-in Lambert term with two soft bands, so the shader
// costs a few instructions per pixel and no draw call or vertex at all. There is no outline: the
// silhouette is the mesh itself.
//
// The color comes from the vertex color times `albedo` (the theme palette), never from a texture.
// `band_softness` is the half-width of each band edge in N·L, so the transition is soft instead of
// a hard posterized step. The lit band is slightly warm and the shade band slightly lilac; the
// lilac of the shade also comes from the theme's ambient and cold fill (`light.shadow_tint`,
// `light.fill`), so a face in shadow is never dead gray.
//
// A discreet warm rim only on the side facing the sun: the light direction only exists inside
// `light()`, and the warmth of the light color tells the sun (warm) from the moon (cool), so only
// the sun gets a rim. The street lamps keep their own color and a single soft edge instead of the
// sun's two bands, so the pools at night are not banded and do not change the night look.

uniform vec3 albedo : source_color = vec3(1.0, 1.0, 1.0);
uniform bool use_vertex_color = true;
uniform float band_softness = 0.16;
// Multipliers of the direct light, not colors: warm on the sun side, lilac in the shade band.
uniform vec3 lit_tint = vec3(1.05, 1.0, 0.94);
uniform vec3 shade_tint = vec3(0.92, 0.88, 0.97);
uniform vec3 rim_color : source_color = vec3(1.0, 0.78, 0.55);
uniform float rim_strength = 0.20;

void fragment() {
	ALBEDO = use_vertex_color ? albedo * COLOR.rgb : albedo;
	ROUGHNESS = 1.0;
	METALLIC = 0.0;
	SPECULAR = 0.0;
}

void light() {
	// Wrapped terminator: the shade band starts a little past 90 degrees, as in the reference.
	float ndl = dot(NORMAL, LIGHT);
	float wrap = clamp((ndl + 0.35) / 1.35, 0.0, 1.0);
	float low = smoothstep(0.30 - band_softness, 0.30 + band_softness, wrap);
	float high = smoothstep(0.72 - band_softness, 0.72 + band_softness, wrap);
	float bands = low * 0.62 + high * 0.38;

	if (!LIGHT_IS_DIRECTIONAL) {
		// A local light (street lamp) keeps its own color, with one soft edge instead of two bands:
		// a lamp pool is small and steep, and the sun's 0.62 mid band darkened the middle of the
		// pool (measured at 21:30, against the plain shading). This near-flat band also stands in
		// for the specular sheen this shader removes, so the night streets keep their brightness
		// (and their lit windows read as before) without a highlight.
		float local_bands = low * 0.98 + high * 0.02;
		DIFFUSE_LIGHT += local_bands * ATTENUATION * LIGHT_COLOR / PI;
	} else {
		// How warm the light is: 1 for the low sun, 0 for the cool moon, in between for the high sun.
		float warmth = smoothstep(0.0, 0.25, LIGHT_COLOR.r - LIGHT_COLOR.b);
		// The warm/lilac tint belongs to the sun: the moon keeps its own (cool) color, only banded.
		vec3 tint = mix(vec3(1.0), mix(shade_tint, lit_tint, bands), warmth);
		// The shade band keeps a small share of the warm light, tinted lilac, so the side away from
		// the sun reads as painted shade instead of only ambient. Off for the cool lights.
		vec3 shade = tint * max(bands, warmth * 0.10);
		DIFFUSE_LIGHT += shade * ATTENUATION * LIGHT_COLOR / PI;

		// Rim: a thin warm edge where the surface turns away from the camera but still faces the sun.
		// Added to the diffuse light, so it takes the paint color of the surface.
		float fresnel = pow(1.0 - clamp(dot(NORMAL, VIEW), 0.0, 1.0), 3.0);
		float sunward = smoothstep(0.0, 0.35, ndl);
		float rim = rim_strength * warmth * fresnel * sunward;
		DIFFUSE_LIGHT += rim_color * rim * ATTENUATION * LIGHT_COLOR / PI;
	}
}
