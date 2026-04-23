// Vertex shader for all 2D geometry in the game.
//
// Push constants layout in v2:
//
//   offset   field
//    0..8    vec2  scale
//    8..16   vec2  shake_offset  (in game-space units, pre-aspect)
//    16..20  float zoom          (uniform camera zoom, 1.0 = neutral)

#version 450

layout(push_constant) uniform PushConstants {
    vec2  scale;
    vec2  shake;
    float zoom;
} pc;

layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec4 in_color;

layout(location = 0) out vec4 v_color;

void main() {
    vec2 p = (in_pos * pc.zoom) + pc.shake;
    gl_Position = vec4(p.x * pc.scale.x,
                       p.y * pc.scale.y,
                       0.0, 1.0);
    v_color = in_color;
}