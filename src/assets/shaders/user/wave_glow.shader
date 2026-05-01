// Radial ripple distortion plus a soft center glow.
//
// p0 intensity  amplitude of the UV ripple (0..1 reasonable)
// p1 scale      strength of the central glow (0..1)
// p2 freq       ripple density, scaled internally by 40
// p3 unused
//
// The `freq` param is multiplied by 40 inside so an
// authored value of 0.25 produces roughly ten visible
// rings across the screen. This keeps trigger side numbers
// in the intuitive 0..1 range.

shader "wave_glow" {
    param intensity : float = 1.0
    param scale     : float = 0.5
    param freq      : float = 10.0
    param p3        : float = 0.0

    body {
        let ux = dot(uv, vec2(1.0, 0.0))
        let uy = dot(uv, vec2(0.0, 1.0))
        let dx = ux - 0.5
        let dy = uy - 0.5
        let d  = length(vec2(dx, dy))

        let w  = sin(d * freq * 40.0) * 0.015 * intensity
        let nx = ux + dx * w
        let ny = uy + dy * w

        let g  = scale * (1.0 - smoothstep(0.0, 0.5, d)) * 0.25

        output sample(vec2(nx, ny)) + vec4(g, g, g, 0.0)
    }
}