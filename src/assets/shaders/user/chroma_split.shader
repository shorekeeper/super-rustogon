// Radial chromatic aberration. Red and blue channels sample
// at a shifted uv while green stays on the original pixel,
// so the edges of bright areas fringe rainbow colors.
//
// p0 amount  displacement magnitude. Multiplied by 0.015
//            internally so authored 1.0 shifts R and B by
//            about 1.5% of the frame (~20 pixels at 1280
//            wide).
// p1 center  origin of the radial split. 0.5 keeps it at
//            screen middle; shifting moves the null point.
// p2 unused
// p3 unused

shader "chroma_split" {
    param amount : float = 0.6
    param center : float = 0.5
    param p2     : float = 1.0
    param p3     : float = 0.0

    body {
        let ux = dot(uv, vec2(1.0, 0.0))
        let uy = dot(uv, vec2(0.0, 1.0))
        let dx = ux - center
        let dy = uy - center
        let k  = amount * 0.015

        output vec4(
            dot(sample(vec2(ux + dx * k, uy + dy * k)),
                vec4(1.0, 0.0, 0.0, 0.0)),
            dot(sample(uv),
                vec4(0.0, 1.0, 0.0, 0.0)),
            dot(sample(vec2(ux - dx * k, uy - dy * k)),
                vec4(0.0, 0.0, 1.0, 0.0)),
            1.0
        )
    }
}