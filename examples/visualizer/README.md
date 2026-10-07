# Spectrum (example theme)

Shows every theme layer:

1. `theme.json` holds the tokens: neon colors, radius and font.
2. `layout.json` puts the player bar on top and hides the right panel.
3. `theme.css` adds the glow and gradient titles. `components/media-card.html` overrides the card template.
4. `script.js` draws a live spectrum behind the player bar and flashes on each beat (`mp3.audio.subscribe`). It also adds a full-screen "visualizer" view and a Ctrl+K command, and remembers the chosen style in `mp3.storage`.

To build it, zip the folder contents into `Spectrum.theme`: run `node scripts/pack-theme.mjs examples/visualizer examples/Spectrum.theme`.
