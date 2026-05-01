// CRT style scanlines with adjustable density and strength.
//
// p0 strength  how dark the troughs of the scanline wave get
// p1 lines     angular frequency of the scanline pattern in uv
//              space. 240 gives ~38 cycles across a 720p frame.
// p2 offset    ambient light added to the darkest scanline to
//              stop the image going fully black on strong
//              settings. 0.0 is pitch dark, 1.0 disables.
// p3 unused

shader "scanline_pro" {
    param strength : float = 0.8
    param lines    : float = 240.0
    param offset   : float = 0.15
    param p3       : float = 0.0

    body {
        let uy = dot(uv, vec2(0.0, 1.0))
        let s  = sin(uy * lines) * 0.5 + 0.5
        let k  = 1.0 - strength * (1.0 - s) * (1.0 - offset)

        output sample(uv) * vec4(k, k, k, 1.0)
    }
}