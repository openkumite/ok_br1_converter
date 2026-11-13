#version 300 es
// PS1-look vertex shader: integer screen-space snapping + affine UVs.
precision highp float;

uniform mat4 uModelViewProjection;
uniform vec2 uSnapResolution; // e.g. 320x240

in vec3 aPosition;
in vec2 aUv;
in vec4 aColor;

out vec3 vAffine; // (uv*w, w): dividing per pixel gives affine mapping
out vec4 vColor;

void main() {
    vec4 clip = uModelViewProjection * vec4(aPosition, 1.0);
    vec2 ndc = clip.xy / clip.w;
    ndc = floor(ndc * uSnapResolution * 0.5 + 0.5) / (uSnapResolution * 0.5);
    gl_Position = vec4(ndc * clip.w, clip.zw);
    vAffine = vec3(aUv * clip.w, clip.w);
    vColor = aColor;
}
