//! Putting every image in one frame: pick a reference, chain the pairwise homographies along a
//! spanning tree, then refine each image against all of its neighbours.

use super::geom::{self, H, IDENTITY, P};

/// Two images that overlap: `h` takes points of image `i` to image `j`, and `pairs` are the
/// matched points (in `i`, in `j`) that agree with it.
pub struct Edge {
    pub i: usize,
    pub j: usize,
    pub h: H,
    pub pairs: Vec<(P, P)>,
}

pub struct Layout {
    pub reference: usize,
    /// Per image: the map to the reference's frame, or none when it isn't connected to it.
    pub h: Vec<Option<H>>,
}

fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

pub fn layout(n: usize, edges: &[Edge]) -> Option<Layout> {
    // The largest group of images that overlap one another.
    let mut parent: Vec<usize> = (0..n).collect();
    for e in edges {
        let (a, b) = (find(&mut parent, e.i), find(&mut parent, e.j));
        parent[a] = b;
    }
    let mut size = vec![0usize; n];
    let mut weight = vec![0usize; n];
    for i in 0..n {
        let r = find(&mut parent, i);
        size[r] += 1;
    }
    for e in edges {
        let r = find(&mut parent, e.i);
        weight[r] += e.pairs.len();
    }
    let root = (0..n).filter(|&r| size[r] > 1).max_by_key(|&r| (size[r], weight[r]))?;
    let member = |parent: &mut [usize], i: usize| find(parent, i) == root;
    let edges: Vec<&Edge> = edges.iter().filter(|e| member(&mut parent, e.i)).collect();

    // The reference is the image with the most matches: the middle of the mosaic.
    let mut degree = vec![0usize; n];
    for e in &edges {
        degree[e.i] += e.pairs.len();
        degree[e.j] += e.pairs.len();
    }
    let reference = (0..n).filter(|&i| degree[i] > 0).max_by_key(|&i| degree[i])?;

    // Maximum spanning tree from the reference, composing homographies as it grows.
    let mut h: Vec<Option<H>> = vec![None; n];
    h[reference] = Some(IDENTITY);
    let mut order = vec![reference];
    loop {
        let next = edges.iter().filter(|e| h[e.i].is_some() != h[e.j].is_some()).max_by_key(|e| e.pairs.len());
        let Some(e) = next else { break };
        if let Some(hj) = h[e.j] {
            h[e.i] = Some(geom::normalized(&geom::mul(&hj, &e.h)));
            order.push(e.i);
        } else if let (Some(hi), Some(inv)) = (h[e.i], geom::inv(&e.h)) {
            h[e.j] = Some(geom::normalized(&geom::mul(&hi, &inv)));
            order.push(e.j);
        } else {
            break;
        }
    }

    // Each image in turn is re-fitted to where its neighbours currently put its matches. The
    // chain accumulates error along the tree; this spreads it over every overlap.
    for _ in 0..8 {
        for &k in order.iter().skip(1) {
            let mut pairs: Vec<(P, P)> = Vec::new();
            for e in &edges {
                let (neighbour, k_is_j) = if e.i == k {
                    (e.j, false)
                } else if e.j == k {
                    (e.i, true)
                } else {
                    continue;
                };
                let Some(hn) = h[neighbour] else { continue };
                for &(a, b) in &e.pairs {
                    // `a` is in image i, `b` in image j: the neighbour's point lands in the frame.
                    let (theirs, mine) = if k_is_j { (a, b) } else { (b, a) };
                    if let Some(target) = geom::apply(&hn, theirs.0, theirs.1) {
                        pairs.push((mine, target));
                    }
                }
            }
            if let Some(fit) = geom::dlt(&pairs) {
                h[k] = Some(fit);
            }
        }
    }
    Some(Layout { reference, h })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_of_translations_is_composed() {
        // Image 0 and 1 are 100 px apart, 1 and 2 another 100: 0 → 2 is 200.
        let pts: Vec<P> = (0..6).flat_map(|i| (0..5).map(move |j| (30.0 * i as f64, 25.0 * j as f64))).collect();
        let shift = |dx: f64| -> [f64; 9] { [1.0, 0.0, dx, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] };
        let edge = |i, j, dx: f64| Edge { i, j, h: shift(dx), pairs: pts.iter().map(|&(x, y)| ((x, y), (x + dx, y))).collect() };
        let l = layout(3, &[edge(0, 1, 100.0), edge(1, 2, 100.0)]).unwrap();
        assert_eq!(l.reference, 1);
        let at = |i: usize| geom::apply(&l.h[i].unwrap(), 0.0, 0.0).unwrap();
        assert!((at(0).0 - 100.0).abs() < 1e-6 && (at(2).0 + 100.0).abs() < 1e-6 && at(1).0.abs() < 1e-9);
    }

    #[test]
    fn a_lone_image_is_left_out() {
        let pts: Vec<P> = (0..6).flat_map(|i| (0..5).map(move |j| (30.0 * i as f64, 25.0 * j as f64))).collect();
        let e = Edge { i: 0, j: 1, h: IDENTITY, pairs: pts.iter().map(|&p| (p, p)).collect() };
        let l = layout(3, &[e]).unwrap();
        assert!(l.h[2].is_none() && l.h[0].is_some() && l.h[1].is_some());
    }
}
