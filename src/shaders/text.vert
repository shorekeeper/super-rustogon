// Vertex shader for SDF text rendering.
//
// One push constant carries the per frame aspect correction so
// the text pipeline sees the same coordinate convention as the
// main pipeline. Vertex inputs are interleaved: position in game
// space coordinates (matches Vertex), then atlas UV in 0..1, then
// RGBA color. The fragment shader reads the SDF atlas through a
// combined image sampler bound to set 0 binding 0.

#version 450

layout(push_constant) uniform PushConstants {
    vec2 scale;
} pc;

layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_color;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;

void main() {
    gl_Position = vec4(in_pos.x * pc.scale.x,
                       in_pos.y * pc.scale.y,
                       0.0, 1.0);
    v_uv = in_uv;
    v_color = in_color;
}