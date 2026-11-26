//! Design ▸ Re-Layout Page (Visio's automatic layout, the flowchart and
//! circular styles): place the page's shapes from the connector graph.
//! Layered layout: longest-path layers from the sources (cycles broken
//! at back edges), two barycentre passes to order each layer, then the
//! layers stacked in the chosen direction with the given spacing,
//! centred on the page. Unconnected shapes go in a last row.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    TopToBottom,
    BottomToTop,
    LeftToRight,
    RightToLeft,
    Circular,
}

/// One node: id and size (inches).
#[derive(Debug, Clone, Copy)]
pub struct Node {
    pub id: i64,
    pub w: f64,
    pub h: f64,
}

/// New pin (centre) per node, page inches y-up, centred on `centre`.
pub fn layout(nodes: &[Node], edges: &[(i64, i64)], style: Style, gap: f64, centre: (f64, f64)) -> HashMap<i64, (f64, f64)> {
    let mut out = HashMap::new();
    if nodes.is_empty() {
        return out;
    }
    let idx: HashMap<i64, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    let n = nodes.len();
    let mut succ = vec![Vec::new(); n];
    let mut connected = vec![false; n];
    for &(a, b) in edges {
        if let (Some(&i), Some(&j)) = (idx.get(&a), idx.get(&b)) {
            if i != j && !succ[i].contains(&j) {
                succ[i].push(j);
                connected[i] = true;
                connected[j] = true;
            }
        }
    }
    if style == Style::Circular {
        let k: Vec<usize> = (0..n).collect();
        let span: f64 = k.iter().map(|&i| nodes[i].w.max(nodes[i].h) + gap).sum();
        let r = (span / std::f64::consts::TAU).max(1.0);
        for (j, &i) in k.iter().enumerate() {
            let a = std::f64::consts::FRAC_PI_2 - j as f64 * std::f64::consts::TAU / n as f64;
            out.insert(nodes[i].id, (centre.0 + r * a.cos(), centre.1 + r * a.sin()));
        }
        return out;
    }
    // break cycles: drop edges that close a DFS back edge
    let mut state = vec![0u8; n];
    let mut dag = vec![Vec::new(); n];
    fn dfs(v: usize, succ: &[Vec<usize>], state: &mut [u8], dag: &mut [Vec<usize>]) {
        state[v] = 1;
        for &w in &succ[v] {
            match state[w] {
                0 => {
                    dag[v].push(w);
                    dfs(w, succ, state, dag);
                }
                2 => dag[v].push(w),
                _ => {} // back edge
            }
        }
        state[v] = 2;
    }
    for v in 0..n {
        if state[v] == 0 && connected[v] {
            dfs(v, &succ, &mut state, &mut dag);
        }
    }
    // longest-path layers
    let mut layer = vec![0usize; n];
    let mut changed = true;
    let mut guard = 0;
    while changed && guard < n + 1 {
        changed = false;
        guard += 1;
        for v in 0..n {
            for &w in &dag[v] {
                if layer[w] < layer[v] + 1 {
                    layer[w] = layer[v] + 1;
                    changed = true;
                }
            }
        }
    }
    let max_layer = (0..n).filter(|&v| connected[v]).map(|v| layer[v]).max().unwrap_or(0);
    for v in 0..n {
        if !connected[v] {
            layer[v] = max_layer + 1;
        }
    }
    let layers_n = layer.iter().max().copied().unwrap_or(0) + 1;
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); layers_n];
    for v in 0..n {
        rows[layer[v]].push(v);
    }
    // barycentre ordering, down then up
    let mut pos = vec![0.0f64; n];
    let set_pos = |rows: &Vec<Vec<usize>>, pos: &mut Vec<f64>| {
        for r in rows {
            for (i, &v) in r.iter().enumerate() {
                pos[v] = i as f64;
            }
        }
    };
    set_pos(&rows, &mut pos);
    let mut pred = vec![Vec::new(); n];
    for (v, outs) in dag.iter().enumerate() {
        for &w in outs {
            pred[w].push(v);
        }
    }
    for pass in 0..4 {
        let order: Vec<usize> = if pass % 2 == 0 { (1..layers_n).collect() } else { (0..layers_n.saturating_sub(1)).rev().collect() };
        for l in order {
            let nb = |v: usize| if pass % 2 == 0 { &pred[v] } else { &dag[v] };
            let mut keyed: Vec<(f64, usize)> = rows[l]
                .iter()
                .map(|&v| {
                    let ns = nb(v);
                    let k = if ns.is_empty() { pos[v] } else { ns.iter().map(|&u| pos[u]).sum::<f64>() / ns.len() as f64 };
                    (k, v)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
            rows[l] = keyed.into_iter().map(|(_, v)| v).collect();
            for (i, &v) in rows[l].iter().enumerate() {
                pos[v] = i as f64;
            }
        }
    }
    // coordinates: along = across the layer, depth = layer axis
    let vertical = matches!(style, Style::TopToBottom | Style::BottomToTop);
    let size = |v: usize| if vertical { (nodes[v].w, nodes[v].h) } else { (nodes[v].h, nodes[v].w) };
    let layer_depth: Vec<f64> = rows.iter().map(|r| r.iter().map(|&v| size(v).1).fold(0.0, f64::max)).collect();
    let total_depth: f64 = layer_depth.iter().sum::<f64>() + gap * 1.5 * (layers_n.saturating_sub(1)) as f64;
    let mut d = -total_depth / 2.0;
    for (l, r) in rows.iter().enumerate() {
        let width: f64 = r.iter().map(|&v| size(v).0).sum::<f64>() + gap * r.len().saturating_sub(1) as f64;
        let mut a = -width / 2.0;
        let dc = d + layer_depth[l] / 2.0;
        for &v in r {
            let ac = a + size(v).0 / 2.0;
            let (x, y) = match style {
                Style::TopToBottom => (ac, -dc),
                Style::BottomToTop => (ac, dc),
                Style::LeftToRight => (dc, -ac),
                Style::RightToLeft => (-dc, -ac),
                Style::Circular => unreachable!(),
            };
            out.insert(nodes[v].id, (centre.0 + x, centre.1 + y));
            a += size(v).0 + gap;
        }
        d += layer_depth[l] + gap * 1.5;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_and_branch_lay_out_in_layers() {
        let nodes: Vec<Node> = (1..=4).map(|id| Node { id, w: 1.0, h: 0.5 }).collect();
        // 1 -> 2 -> 4, 1 -> 3, plus a cycle edge 4 -> 1 that must not loop
        let edges = [(1, 2), (2, 4), (1, 3), (4, 1)];
        let p = layout(&nodes, &edges, Style::TopToBottom, 0.5, (0.0, 0.0));
        assert!(p[&1].1 > p[&2].1 && p[&2].1 > p[&4].1, "top to bottom: {p:?}");
        assert!((p[&2].1 - p[&3].1).abs() < 1e-9, "2 and 3 share a layer");
        let lr = layout(&nodes, &edges, Style::LeftToRight, 0.5, (0.0, 0.0));
        assert!(lr[&1].0 < lr[&2].0 && lr[&2].0 < lr[&4].0);
        let c = layout(&nodes, &edges, Style::Circular, 0.5, (0.0, 0.0));
        let r: Vec<f64> = c.values().map(|(x, y)| x.hypot(*y)).collect();
        assert!(r.iter().all(|v| (v - r[0]).abs() < 1e-9), "on one circle");
    }
}
