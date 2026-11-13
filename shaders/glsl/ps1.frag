#version 300 es
// PS1-look fragment shader: texture * vertex colour (exported with 0x80 = 1.0),
// 0x0000 texels transparent, 15-bit colour with 4x4 ordered dither.
precision mediump float;

uniform sampler2D uTexture; // sample with GL_NEAREST
uniform bool uTextured;

in vec3 vAffine;
in vec4 vColor;
out vec4 fragColor;

const mat4 BAYER = mat4(-4., 0., -3., 1., 2., -2., 3., -1., -3., 1., -4., 0., 3., -1., 2., -2.);

void main() {
    vec4 c = vColor;
    if (uTextured) {
        vec4 t = texture(uTexture, vAffine.xy / vAffine.z);
        if (t.a < 0.5) discard;
        c = vec4(t.rgb * vColor.rgb, t.a); // okconv-br1 exports colours with 0x80 = 1.0
    }
    ivec2 p = ivec2(gl_FragCoord.xy) & 3;
    vec3 rgb = c.rgb * 255.0 + BAYER[p.x][p.y];
    fragColor = vec4(floor(clamp(rgb, 0.0, 255.0) / 8.0) * 8.0 / 255.0, c.a);
}
