// Fullscreen triangle vertex shader for the post-process pass.
//
// No vertex buffer, no input bindings. The host draws exactly three
// vertices. Positions and UVs are generated from gl_VertexIndex
// using the standard "big triangle covers the viewport" trick:
//
//   index 0 -> uv (0, 0), pos (-1, -1)
//   index 1 -> uv (2, 0), pos ( 3, -1)
//   index 2 -> uv (0, 2), pos (-1,  3)
//
// The triangle extends past the viewport; the rasterizer clips it
// to the unit square and the interpolated UV on that square runs
// from (0, 0) at the top-left to (1, 1) at the bottom-right, which
// matches Vulkan's framebuffer coordinate convention.

#version 450

layout(location = 0) out vec2 v_uv;

void main() {
    vec2 uv = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    v_uv = uv;
    gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}