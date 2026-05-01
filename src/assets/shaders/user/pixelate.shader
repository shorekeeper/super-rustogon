// Quantize UVs to a coarse grid so the scene reads as a
// low resolution bitmap. Useful for retro drops.
//
// p0 blocks  number of grid cells per axis. 8 is extremely
//            chunky, 40 is moderate, 80 starts to look like
//            a CRT scanline mesh.
// p1..p3  unused

shader "pixelate" {
    param blocks : float = 8.0
    param p1     : float = 0.0
    param p2     : float = 0.0
    param p3     : float = 0.0

    body {
        let ux = dot(uv, vec2(1.0, 0.0))
        let uy = dot(uv, vec2(0.0, 1.0))
        let bx = floor(ux * blocks) / blocks
        let by = floor(uy * blocks) / blocks

        output sample(vec2(bx, by))
    }
}