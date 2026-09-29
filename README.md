# Mindsweeper — a principled take on minesweeper

> This is a fork of [alexbuz/mindsweeper](https://github.com/alexbuz/mindsweeper) (GPL-3.0) that adds a
> **Pattern coach**: after a mistake it names the minesweeper pattern you missed and explains it
> (see feature 8 below). Everything else is the original game.

To play, visit https://alexbuz.github.io/mindsweeper/. Once the page loads, no further internet
connection is required.

## Background

Traditional minesweeper is a game of logical deduction—until it's not. Sometimes, you end up in a
situation where there is not enough information to find a tile that is definitely safe to reveal. In
such cases, guessing is required to proceed, and that often leads to the loss of an otherwise
smooth-sailing game. However, there's no reason it has to be that way. It's merely a consequence of
the random manner in which mines are typically arranged at the start of the game. Some mine
arrangements happen to necessitate guessing, while others do not. This is a matter of luck, and it's
not a particularly fun aspect of a game that is otherwise about logic.

Eliminating the need for guesswork, then, is a matter of modifying the mine arrangement algorithm.
Rather than simply placing each mine under a random tile, it should instead consider each mine
arrangement as a whole, choosing a random mine arrangement from the set of mine arrangements that
would allow a perfect logician to win without guessing. Ideally, it should sample uniformly from
that set of mine arrangements, ensuring that every such arrangement is equally likely to be chosen.
That is precisely what mindsweeper is designed to do, and it accomplishes this within a matter of
milliseconds after you make your first click.

## Features

1. Guessing is *never* necessary
    - There's no need to toggle a setting. Mindsweeper is a game of pure skill, always.
2. Guess punishment
    - Since guessing is already unnecessary, it's only natural to take the idea of "no guessing" a
      step further and forbid guessing entirely. This feature, enabled by default, effectively rids
      the game of all remaining luck aspects. If you click on a tile that *can* be a mine, then it
      *will* be a mine, guaranteed.
3. Unrestricted first click
    - Mindsweeper does not obligate you to click a particular tile to start the game. The mine
      arrangement algorithm works on demand, and is fast enough to avoid introducing any delay.
4. Uniform sampling
    - All mine arrangements are viable, except those that necessitate guessing. If a particular mine
      arrangement is solvable by a perfect logician without guessing, then it's just as likely to be
      picked by the algorithm as every other viable arrangement.
5. Post-mortem analysis
    - If you reveal a mine and lose the game, you'll get feedback that helps you improve. You get to
      see which flags you misplaced (if any), as well as which tiles you could (and could not) have
      safely revealed. Tiles are color-coded to show this information at a glance, and you can also
      hover over any tile to see this explained in words.
6. High performance
    - Mindsweeper is written in Rust and compiles to WASM. When you make your first click, the mine
      arrangement algorithm generally finishes running before you even release the mouse button, so
      there is no first-click delay.
7. Completely offline
    - Mindsweeper does not depend on a server. All of the code runs locally in your browser.
8. Pattern coach
    - After a loss (click any unrevealed tile), a side panel names the pattern that would have
      proven that tile safe or a mine (1-1, 1-2, 1-2-1, 1-2-2-1, overlap, mine
      counting, "what-if" contradiction, ...), highlights the numbers involved with A/B/C/D labels,
      explains the reasoning in words, and lists other tiles the same pattern would have solved.
      Reductions (e.g. 1-1R) are worked out on the "mines still needed" count.
    - After a loss, **Undo click** restores the board to just before the losing click so you can
      keep playing with the explanation still on screen. The clock resumes, and the game no longer
      counts for best times.
    - The **Hint** button gives a nudge first ("look here"), then the full explanation. Using it
      disables best-time recording for that game.
    - Every explanation is derived only from the visible numbers, and is tested against the
      exhaustive solver for soundness (`cargo test explain`). "Overlap" is the general two-numbers rule behind the C-shapes, holes and triangles. Patterns beyond the named ones are
      shown as generic overlap / what-if reasoning.

## Building from source

Install [Trunk](https://trunkrs.dev/), and then run `trunk build` in the project directory:

```sh
git clone https://github.com/alexbuz/mindsweeper.git
cd mindsweeper
trunk build
```

The built files will be placed in the `dist` directory, the contents of which must be served to the
user.

For development, instead of `trunk build`, you can run `trunk serve --public-url=/ --open` to start
a local server that automatically rebuilds the project when you make changes.
