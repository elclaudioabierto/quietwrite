# Title

I built QuietWrite: a tiny Rust writing environment for distraction-free writer decks

# Post

I wanted a writing tool that felt more like opening a notebook than launching an application, so I built QuietWrite.

It is a small, keyboard-first Rust app organized around focused writing spaces:

- **Notes** for fragments, ideas, and daily pages
- **Drafts** for longer pieces currently in progress
- **Journal** for dated personal writing
- **Poems** for work that deserves its own shelf
- **Ideas** for premises, images, titles, and seeds
- **Projects** for ordered KDP book manuscripts, chapters, and one-key HTML/Markdown export
- **Secret Thoughts** for locally encrypted private writing
- **Archive** for finished or dormant work

Ordinary shelves are saved as portable Markdown under `~/Writing`; Secret Thoughts use authenticated local encryption. There is no account, cloud dependency, telemetry, NLP, or “AI writing assistant.”

It autosaves, starts in a terminal, supports high-contrast light and dark themes, and has a direct-framebuffer mode for the Raspberry Pi Zero W. That last part is the heart of the project: QuietWrite can turn modest hardware into a focused, instant-on writing deck without requiring a desktop environment.

Secret Thoughts are encrypted with a password-derived key, including their local version history. Filenames and filesystem metadata are not encrypted, and forgotten passwords cannot be recovered.

I’m trying to make this feel less like a text editor with features and more like a small place you want to return to and write. I’d especially value feedback from this community on the shelf model, keyboard flow, readability, and what makes a dedicated writer deck feel genuinely calm.

GitHub: https://github.com/elclaudioabierto/quietwrite

Built with Rust, Ratatui, and Crossterm. MIT licensed.
