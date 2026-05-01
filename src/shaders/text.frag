// SDF fragment shader.
//
// The atlas stores one signed distance value per pixel in the
// red channel of an R8_UNORM texture. Encoding convention:
//
//   stored = clamp(signed_dist_pixels / SDF_SPREAD, -1, 1)
//          * 0.5 + 0.5
//
// so 0.5 is exactly on the contour, values above 0.5 are inside
// the glyph (visible), values below are outside (transparent).
//
// fwidth(dist) gives the magnitude of the distance derivative
// across one screen pixel, which is the natural width of an
// anti aliased edge regardless of how the text is scaled. The
// max() floor keeps the AA from collapsing to zero on very
// large text where derivatives are close to zero per pixel.

#version 450

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform sampler2D u_atlas;

void main() {
    float dist = texture(u_atlas, v_uv).r;
    float aa = max(fwidth(dist), 0.005);
    float alpha = smoothstep(0.5 - aa, 0.5 + aa, dist);
    out_color = vec4(v_color.rgb, v_color.a * alpha);
}