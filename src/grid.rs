//! Spatial index: a uniform grid over boxes in world coordinates.

/// Uniform grid over boxes in world coordinates, CSR layout.
pub struct Grid {
    x0: f64,
    y0: f64,
    cell: f64,
    nx: usize,
    ny: usize,
    offsets: Vec<u32>,
    items: Vec<u32>,
}

impl Grid {
    /// `boxes[i]` is `[min_x, min_y, max_x, max_y]` of item `i` inside `bounds`, `cells` the number
    /// of cells along the longer side of `bounds`.
    pub fn build(boxes: &[[f64; 4]], bounds: [f64; 4], cells: f64) -> Self {
        let span = (bounds[2] - bounds[0]).max(bounds[3] - bounds[1]);
        let cell = if span > 0.0 { span / cells } else { 1e-9 };
        let nx = (((bounds[2] - bounds[0]) / cell).ceil() as usize).max(1);
        let ny = (((bounds[3] - bounds[1]) / cell).ceil() as usize).max(1);
        let mut grid = Self {
            x0: bounds[0],
            y0: bounds[1],
            cell,
            nx,
            ny,
            offsets: vec![0; nx * ny + 1],
            items: Vec::new(),
        };
        for b in boxes {
            let (x0, y0, x1, y1) = grid.range(*b);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    grid.offsets[y * nx + x + 1] += 1;
                }
            }
        }
        for i in 1..grid.offsets.len() {
            grid.offsets[i] += grid.offsets[i - 1];
        }
        let mut fill = grid.offsets.clone();
        grid.items = vec![0; *grid.offsets.last().unwrap_or(&0) as usize];
        for (i, b) in boxes.iter().enumerate() {
            let (x0, y0, x1, y1) = grid.range(*b);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let slot = &mut fill[y * nx + x];
                    grid.items[*slot as usize] = i as u32;
                    *slot += 1;
                }
            }
        }
        grid
    }

    /// Cell index range covered by a box, clamped to the grid.
    fn range(&self, b: [f64; 4]) -> (usize, usize, usize, usize) {
        let cx = |x: f64| (((x - self.x0) / self.cell).floor().max(0.0) as usize).min(self.nx - 1);
        let cy = |y: f64| (((y - self.y0) / self.cell).floor().max(0.0) as usize).min(self.ny - 1);
        (cx(b[0]), cy(b[1]), cx(b[2]), cy(b[3]))
    }

    /// Sorted, unique items whose cells intersect the box.
    pub fn query(&self, b: [f64; 4]) -> Vec<u32> {
        if self.items.is_empty() || b[2] < self.x0 || b[3] < self.y0 {
            return Vec::new();
        }
        let x_end = self.x0 + self.nx as f64 * self.cell;
        let y_end = self.y0 + self.ny as f64 * self.cell;
        if b[0] > x_end || b[1] > y_end {
            return Vec::new();
        }
        let (x0, y0, x1, y1) = self.range(b);
        let mut out = Vec::new();
        for y in y0..=y1 {
            let row = y * self.nx;
            let (from, to) = (
                self.offsets[row + x0] as usize,
                self.offsets[row + x1 + 1] as usize,
            );
            out.extend_from_slice(&self.items[from..to]);
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Bounds of all `boxes`, zero without any.
pub fn bounds(boxes: &[[f64; 4]]) -> [f64; 4] {
    let b = boxes
        .iter()
        .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        });
    if b[0] > b[2] { [0.0; 4] } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_query() {
        let boxes = [
            [0.0, 0.0, 0.1, 0.1],
            [0.5, 0.5, 0.6, 0.6],
            [0.0, 0.0, 1.0, 1.0],
        ];
        let grid = Grid::build(&boxes, bounds(&boxes), 256.0);
        assert_eq!(grid.query([0.0, 0.0, 0.05, 0.05]), vec![0, 2]);
        assert_eq!(grid.query([0.55, 0.55, 0.56, 0.56]), vec![1, 2]);
        assert_eq!(grid.query([-1.0, -1.0, 2.0, 2.0]), vec![0, 1, 2]);
        assert!(grid.query([1.5, 1.5, 2.0, 2.0]).is_empty());
        assert!(
            Grid::build(&[], bounds(&[]), 256.0)
                .query([0.0, 0.0, 1.0, 1.0])
                .is_empty()
        );
    }
}
