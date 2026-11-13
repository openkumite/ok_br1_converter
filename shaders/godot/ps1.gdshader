// PS1-look spatial shader for Godot 4 (use on meshes imported from okconv-br1 .glb files).
// Vertex snapping (integer GTE screen coords), affine texture mapping, vertex colours,
// 15-bit colour with 4x4 ordered dither, nearest-neighbour sampling, unshaded.
shader_type spatial;
render_mode unshaded, cull_disabled, depth_draw_opaque;

uniform sampler2D albedo_tex : source_color, filter_nearest, repeat_disable;
uniform vec4 albedo : source_color = vec4(1.0);
uniform vec2 snap_resolution = vec2(320.0, 240.0);
uniform bool affine_mapping = true;
uniform bool dithering = true;
uniform float alpha_cutoff : hint_range(0.0, 1.0) = 0.5;

varying vec2 affine_uv;
varying float affine_w;

void vertex() {
	vec4 clip = PROJECTION_MATRIX * MODELVIEW_MATRIX * vec4(VERTEX, 1.0);
	vec2 ndc = clip.xy / clip.w;
	ndc = floor(ndc * snap_resolution * 0.5 + 0.5) / (snap_resolution * 0.5);
	clip.xy = ndc * clip.w;
	POSITION = clip;
	// Pre-multiply by w; dividing in fragment() undoes perspective correction.
	affine_w = clip.w;
	affine_uv = UV * clip.w;
}

void fragment() {
	vec2 uv = affine_mapping ? affine_uv / affine_w : UV;
	vec4 tex = texture(albedo_tex, uv);
	vec4 c = tex * albedo * COLOR;
	if (c.a < alpha_cutoff) {
		discard;
	}
	vec3 rgb = c.rgb * 255.0;
	if (dithering) {
		const float bayer[16] = float[](-4.0, 0.0, -3.0, 1.0, 2.0, -2.0, 3.0, -1.0, -3.0, 1.0, -4.0, 0.0, 3.0, -1.0, 2.0, -2.0);
		ivec2 p = ivec2(FRAGCOORD.xy) & 3;
		rgb += bayer[p.y * 4 + p.x];
	}
	ALBEDO = floor(clamp(rgb, 0.0, 255.0) / 8.0) * 8.0 / 255.0;
}
