//! A multi-layer grid of [`Tile`]s — the document the editor paints into and
//! the input the compiler reads.

use crate::tile::{Direction, Tile};

/// A cell address: which `layer`, and the in-plane `(x, y)`.
///
/// `Copy` and cheap to hash so it works as a map key throughout the crate.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Cell {
    pub layer: u32,
    pub x: u32,
    pub y: u32,
}

impl Cell {
    pub fn new(layer: u32, x: u32, y: u32) -> Self {
        Self { layer, x, y }
    }
}

/// A stack of identically sized grids. Layer 0 is the bottom; higher layers sit
/// above it and connect only through [`Tile::Via`] stacks.
#[derive(Clone, Debug)]
pub struct Board {
    layers: u32,
    width: u32,
    height: u32,
    /// Row-major within a layer, layers outermost:
    /// `tiles[(layer * height + y) * width + x]`.
    tiles: Vec<Tile>,
}

impl Board {
    /// An empty board of the given dimensions. Panics if any dimension is zero.
    pub fn new(layers: u32, width: u32, height: u32) -> Self {
        assert!(layers > 0 && width > 0 && height > 0, "board dimensions must be non-zero");
        let len = layers as usize * width as usize * height as usize;
        Self { layers, width, height, tiles: vec![Tile::Empty; len] }
    }

    pub fn layers(&self) -> u32 {
        self.layers
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Whether a cell lies inside the grid.
    pub fn contains(&self, cell: Cell) -> bool {
        cell.layer < self.layers && cell.x < self.width && cell.y < self.height
    }

    fn index(&self, cell: Cell) -> Option<usize> {
        self.contains(cell).then(|| {
            (cell.layer as usize * self.height as usize + cell.y as usize) * self.width as usize
                + cell.x as usize
        })
    }

    /// The tile at `cell`, or [`Tile::Empty`] for any out-of-bounds address.
    pub fn get(&self, cell: Cell) -> Tile {
        self.index(cell).map_or(Tile::Empty, |i| self.tiles[i])
    }

    /// Overwrite `cell`. Out-of-bounds writes are ignored.
    pub fn set(&mut self, cell: Cell, tile: Tile) {
        if let Some(i) = self.index(cell) {
            self.tiles[i] = tile;
        }
    }

    /// The in-plane neighbour of `cell` in `dir`, if it is on the board.
    pub fn neighbour(&self, cell: Cell, dir: Direction) -> Option<Cell> {
        let (dx, dy) = dir.delta();
        let x = cell.x as i64 + dx as i64;
        let y = cell.y as i64 + dy as i64;
        if x < 0 || y < 0 {
            return None;
        }
        let next = Cell::new(cell.layer, x as u32, y as u32);
        self.contains(next).then_some(next)
    }

    /// The cells directly above and below `cell` that exist on the board.
    pub fn vertical_neighbours(&self, cell: Cell) -> impl Iterator<Item = Cell> + '_ {
        [cell.layer.checked_sub(1), cell.layer.checked_add(1)]
            .into_iter()
            .flatten()
            .map(move |layer| Cell::new(layer, cell.x, cell.y))
            .filter(|&c| self.contains(c))
    }

    /// Every non-empty cell with its tile, in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = (Cell, Tile)> + '_ {
        let (w, h) = (self.width, self.height);
        self.tiles.iter().enumerate().filter_map(move |(i, &tile)| {
            if tile == Tile::Empty {
                return None;
            }
            let i = i as u32;
            let layer = i / (w * h);
            let rem = i % (w * h);
            Some((Cell::new(layer, rem % w, rem / w), tile))
        })
    }

    /// Number of non-empty cells — handy for tests and HUD readouts.
    pub fn tile_count(&self) -> usize {
        self.tiles.iter().filter(|t| **t != Tile::Empty).count()
    }
}
