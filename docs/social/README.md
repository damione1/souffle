# Social assets

Everything here is generated from the live marketing site, never drawn
separately: the poster frame, the type and the app plate all come out of
`site/`, so these files cannot drift from what the site looks like.

Regenerate with `tools/social-assets/make-all.sh` (gitignored; the durable
copy of the pipeline is in the Obsidian vault under
`Projects/Souffle/assets/social-assets-script`).

| File | Size | Where it goes |
| --- | --- | --- |
| `github-social-preview.png` | 1280×640 | Repo **Settings → General → Social preview**. Upload by hand: GitHub has no API for it. |
| `square-en.png`, `square-fr.png` | 1080×1080 | Attached as-is in Teams, Slack, Zoom or Discord, where a link does not unfurl. |
| `poster-meeting-square.gif`, `poster-overlay-square.gif` | 1080×1080 | Animated square posts: the app printed on the amber sheet, headline included. |
| `poster-meeting-vertical.gif`, `poster-overlay-vertical.gif` | 1080×1350 | The same, in the 4:5 feed ratio. |
| `app-meeting-square.gif`, `app-overlay-square.gif` | 1080×1080 | The app alone, edge to edge, no sheet and no headline. |
| `app-meeting-vertical.gif`, `app-overlay-vertical.gif` | 1080×1350 | The same, in 4:5. |

The poster set carries the message on its own and works as a standalone
post. The app set is the raw screen, for a post whose words are in the
caption. The chat scene has two messages in it, so its app-only 4:5 frame
runs airier than the meeting one: the poster variant fills that ratio
better.

The two `og-*.png` cards are not here: they are served by the site itself
and live in `site/src/`, wired into `base.njk` as `og:image`.

`../demo/` is a different set with a different job: the README's own
animations, at the 820px they are displayed at so they stay sharp.
