// Fragment shader: per vertex RGBA color, no textures yet.
// Alpha is interpolated across the triangle and blended with the
// color attachment by the pipeline's alpha blending stage
// (src_alpha, one_minus_src_alpha).

#version 450

layout(location = 0) in vec4 v_color;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = v_color;
}