#version 450

layout(push_constant) uniform PushConstants {
    vec2  scale;
    vec2  shake;
    float zoom;
    float tilt_angle;
    float tilt_pitch;
    float tilt_yaw;
} pc;

layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec4 in_color;

layout(location = 0) out vec4 v_color;

void main() {
    vec2 p = (in_pos * pc.zoom) + pc.shake;

    // Z-axis roll. When tilt_angle is zero, cos = 1 and
    // sin = 0, so the matrix collapses to identity.
    float ca = cos(pc.tilt_angle);
    float sa = sin(pc.tilt_angle);
    p = vec2(p.x * ca - p.y * sa,
             p.x * sa + p.y * ca);

    // Fake perspective. `depth` is how far along the tilted
    // ground plane each vertex sits. Coefficient 0.25 gives
    // a visible SH/OH lean at the parser's max authored tilt
    // (30 degrees) while staying well inside safe numerical
    // territory for vertices near the background edge
    // (radius up to ~5). The max() clamp is a defensive
    // backstop that prevents the perspective divide from
    // flipping sign at pathological corner cases where
    // pitch and yaw are both maxed out and the vertex sits
    // at the far bottom left of the playfield.
    //
    // At tilt_pitch = 0 and tilt_yaw = 0 both sp and sy are
    // zero, so depth is zero, w is exactly 1.0, and the
    // divide leaves p unchanged. No artifacts appear on
    // levels that only use the angle axis of `:tilt`.
    float sp = sin(pc.tilt_pitch);
    float sy = sin(pc.tilt_yaw);
    float depth = p.y * sp + p.x * sy;
    float w = max(1.0 + depth * 0.25, 0.35);
    p.x = p.x / w;
    p.y = p.y / w;

    gl_Position = vec4(p.x * pc.scale.x,
                       p.y * pc.scale.y,
                       0.0, 1.0);
    v_color = in_color;
}