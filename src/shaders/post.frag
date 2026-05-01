#version 450

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform sampler2D u_scene;

layout(push_constant) uniform Params {
    float time;
    float bloom_intensity;
    float chromatic_strength;
    float vignette;
    float scanlines;
    float film_grain;
    float colorblind_mode;
    float high_contrast;
    vec2  resolution;
    float glitch;
    float strobe;
    float invert_colors;
    float grayscale;
    float shockwave_progress;
    float shockwave_strength;
    float fog_near;
    float fog_far;
    float outline_amount;
    float _pad;
} pc;

const float PI = 3.14159265359;

vec3 sample_chromatic(vec2 uv, float strength) {
    vec2 dir = uv - 0.5;
    float amp = strength * 0.006;
    vec3 col;
    col.r = texture(u_scene, uv + dir * amp).r;
    col.g = texture(u_scene, uv).g;
    col.b = texture(u_scene, uv - dir * amp).b;
    return col;
}

vec3 gather_bloom(vec2 uv) {
    const int   SAMPLES = 32;
    const float GOLDEN  = 2.39996323;
    const float RADIUS  = 0.045;
    const float THRESH  = 0.75;
    vec3  accum = vec3(0.0);
    float total = 0.0;
    for (int i = 0; i < SAMPLES; i++) {
        float fi = float(i) + 0.5;
        float t  = fi / float(SAMPLES);
        float r  = sqrt(t) * RADIUS;
        float a  = fi * GOLDEN;
        vec2  off = vec2(cos(a), sin(a)) * r;
        float w = 1.0 - t;
        vec3 s    = texture(u_scene, uv + off).rgb;
        float lum = dot(s, vec3(0.2126, 0.7152, 0.0722));
        float br  = max(lum - THRESH, 0.0);
        accum += s * br * w;
        total += w;
    }
    return accum / total;
}

mat3 colorblind_matrix(int mode) {
    if (mode == 1) {
        return transpose(mat3(
            0.567, 0.433, 0.000,
            0.558, 0.442, 0.000,
            0.000, 0.242, 0.758
        ));
    } else if (mode == 2) {
        return transpose(mat3(
            0.625, 0.375, 0.000,
            0.700, 0.300, 0.000,
            0.000, 0.300, 0.700
        ));
    } else {
        return transpose(mat3(
            0.950, 0.050, 0.000,
            0.000, 0.433, 0.567,
            0.000, 0.475, 0.525
        ));
    }
}

float hash21(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

vec3 apply_glitch(vec2 uv, float g) {
    if (g <= 0.001) return texture(u_scene, uv).rgb;
    float band_h = mix(0.02, 0.05, g);
    float band = floor(uv.y / band_h);
    float seed = band + floor(pc.time * 24.0);
    float jitter = (hash21(vec2(seed, 1.3)) - 0.5) * 0.06 * g;
    float tear_prob = hash21(vec2(seed, 7.7));
    if (tear_prob < 0.15 * g) jitter *= 3.0;
    float chroma = 0.010 * g;
    vec2 off = vec2(jitter, 0.0);
    float r = texture(u_scene, uv + off + vec2( chroma, 0.0)).r;
    float gr= texture(u_scene, uv + off                    ).g;
    float b = texture(u_scene, uv + off + vec2(-chroma, 0.0)).b;
    return vec3(r, gr, b);
}

// Radial shockwave UV displacement. A ring of offset expands
// outward from screen center. Pixels inside the ring get
// pushed inward, pixels outside get pushed outward, giving
// the classic bass-impact ripple.
vec2 apply_shockwave(vec2 uv, float progress, float strength) {
    if (progress <= 0.0 || strength <= 0.001) return uv;
    vec2 c = uv - 0.5;
    float d = length(c);
    float front = progress * 0.85;
    float band  = 0.10;
    float env   = smoothstep(band, 0.0, abs(d - front));
    float push  = sin((d - front) * 40.0) * env * 0.025 * strength
                * (1.0 - progress);
    if (d < 1e-4) return uv;
    return uv + (c / d) * push;
}

// Sobel edge detector on luminance. Returns additive highlight
// color for outlines. Samples the scene texture 8 times around
// the target pixel.
vec3 apply_outline(vec2 uv, float amount) {
    if (amount <= 0.001) return vec3(0.0);
    vec2 px = 1.0 / pc.resolution;
    float tl = dot(texture(u_scene, uv + vec2(-px.x, -px.y)).rgb,
                   vec3(0.2126, 0.7152, 0.0722));
    float tc = dot(texture(u_scene, uv + vec2(  0.0, -px.y)).rgb,
                   vec3(0.2126, 0.7152, 0.0722));
    float tr = dot(texture(u_scene, uv + vec2( px.x, -px.y)).rgb,
                   vec3(0.2126, 0.7152, 0.0722));
    float ml = dot(texture(u_scene, uv + vec2(-px.x,   0.0)).rgb,
                   vec3(0.2126, 0.7152, 0.0722));
    float mr = dot(texture(u_scene, uv + vec2( px.x,   0.0)).rgb,
                   vec3(0.2126, 0.7152, 0.0722));
    float bl = dot(texture(u_scene, uv + vec2(-px.x,  px.y)).rgb,
                   vec3(0.2126, 0.7152, 0.0722));
    float bc = dot(texture(u_scene, uv + vec2(  0.0,  px.y)).rgb,
                   vec3(0.2126, 0.7152, 0.0722));
    float br = dot(texture(u_scene, uv + vec2( px.x,  px.y)).rgb,
                   vec3(0.2126, 0.7152, 0.0722));
    float gx = (tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl);
    float gy = (bl + 2.0 * bc + br) - (tl + 2.0 * tc + tr);
    float mag = sqrt(gx * gx + gy * gy);
    return vec3(mag) * amount * 1.8;
}

void main() {
    vec2 uv = v_uv;

    // Shockwave displaces the UV used to sample the scene,
    // which ripples the entire image including every other
    // effect layered on top.
    uv = apply_shockwave(uv, pc.shockwave_progress, pc.shockwave_strength);

    vec3 col;
    if (pc.glitch > 0.001) {
        col = apply_glitch(uv, pc.glitch);
    } else if (pc.chromatic_strength > 0.001) {
        col = sample_chromatic(uv, pc.chromatic_strength);
    } else {
        col = texture(u_scene, uv).rgb;
    }

    if (pc.bloom_intensity > 0.001) {
        vec3 bloom = gather_bloom(uv);
        col += bloom * pc.bloom_intensity * 1.2;
    }

    // Radial fog. near/far are normalized screen radii 0..1.
    if (pc.fog_far > pc.fog_near + 1e-4) {
        vec2 fc = uv - 0.5;
        float fd = length(fc) * 2.0;
        float fog = smoothstep(pc.fog_near, pc.fog_far, fd);
        col *= 1.0 - fog * 0.85;
    }

    if (pc.vignette > 0.001) {
        vec2 c = uv - 0.5;
        float d = length(c);
        float falloff = smoothstep(0.35, 0.85, d);
        col *= 1.0 - falloff * pc.vignette;
    }

    if (pc.scanlines > 0.5) {
        float line = 0.5 + 0.5 * cos(uv.y * pc.resolution.y * PI);
        col *= mix(0.82, 1.0, line);
    }

    if (pc.film_grain > 0.5) {
        float n = hash21(uv * pc.resolution
                       + vec2(pc.time * 61.3, pc.time * 127.7));
        col += (n - 0.5) * 0.08;
    }

    int cb = int(pc.colorblind_mode + 0.5);
    if (cb > 0) {
        col = colorblind_matrix(cb) * col;
    }

    if (pc.high_contrast > 0.5) {
        col = (col - 0.5) * 1.6 + 0.5;
    }

    if (pc.strobe > 0.001) {
        col = mix(col, vec3(1.0), pc.strobe * 0.6);
    }

    // Outline: Sobel highlight added on top so it reads above
    // bloom and color grading but below global inversions.
    if (pc.outline_amount > 0.001) {
        col += apply_outline(uv, pc.outline_amount);
    }

    // Grayscale mix. Applied before inversion so inverting a
    // grayscale image still yields white on black.
    if (pc.grayscale > 0.001) {
        float lum = dot(col, vec3(0.2126, 0.7152, 0.0722));
        col = mix(col, vec3(lum), pc.grayscale);
    }

    // Color inversion last. Linear interpolation so partial
    // activations produce a crossfade rather than a hard flip.
    if (pc.invert_colors > 0.001) {
        col = mix(col, vec3(1.0) - col, pc.invert_colors);
    }

    {
        float d = hash21(uv * pc.resolution + vec2(17.37, 53.71)) - 0.5;
        col += d * (1.0 / 255.0);
    }

    out_color = vec4(clamp(col, 0.0, 1.0), 1.0);
}