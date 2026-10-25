# Rubyscope web page

The landing page (`index.html`) and the docs (`docs/index.html`, served at
`/docs/`). Plain HTML and CSS, with one small script. No build step.

| Path | What it holds |
|---|---|
| `index.html` | The landing page |
| `docs/index.html` | How to use Rubyscope: mouse, keys, settings |
| `css/site.css` | The styles of both pages |
| `js/site.js` | The header line and the docs contents |
| `fonts/` | Schibsted Grotesk and JetBrains Mono (SIL Open Font License) |
| `img/shots/` | Screenshots of the real window |
| `bin/shots` | Makes the screenshots again |

## Look at it

```
cd webpage
python3 -m http.server 4321
```

Then open http://localhost:4321/ and http://localhost:4321/docs/.

Any static host works: serve the `webpage` folder as the site root.

## Deploy

https://rubyscope.equivalent.work, with Kamal 2, from Linux only. The page
is the `rubyscope-site` service: nginx serves the files, and kamal-proxy
routes `rubyscope.equivalent.work` to it. The proxy is shared with other
apps on the server, so never `kamal proxy reboot` it.

```
cd webpage
kamal deploy       # the first time: kamal setup
```

The server is `deploy.equivalent.work`, a DNS name for its IP address, so
the address stays out of this repo. `rubyscope.equivalent.work` is a CNAME to
that name. Both are DNS only in Cloudflare (no proxy), so kamal-proxy can get
its Let's Encrypt certificate. The health check is `GET /up`. A deploy builds
from the files in this folder, so it needs no commit. `bin/`, `config/` and
the README are not shipped (see `.dockerignore`).

## Screenshots

The screenshots come from the real window, not from a mockup.
`bin/shots` runs the ignored `shots` test in `crates/app/src/shots.rs`. It
writes a made-up shop app to a temp folder, clicks through it with
egui_kittest, and saves each step. Then `bin/crop.py` crops each picture
and saves it as WebP.

```
webpage/bin/shots
```

It needs python3 with Pillow. The shots use the Tokyo Night colors, not
the Omarchy theme of the machine. On a Linux machine without a GPU, install
Mesa's lavapipe driver and point `VK_ICD_FILENAMES` at its
`lvp_icd.json`. The code font is what `fc-match monospace` gives; a Nerd
Font has every symbol the window uses.

When a screenshot changes size, update the `width` and `height` of its
`img` tag.
