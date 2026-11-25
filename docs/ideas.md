# Ideas

I use obsidian and I think I need an extra layer on top of plain markdown editor.

As I use obsidian, it is hard to determine what one file should have.
For instance, should a short idea pop up from my head be a single file?
What is the idea has a chain of another idea that is not exactly classified something within the single file?

I thought the structure that enforces to have a folder to have those organize limits the idea to the structure.

The extra layer that allows me to fix this is having a sqlite.

A markdown file will have a UUIDv7 as name. e.g., `01a03d20-a54c-7977-a1f4-1a88b38855dd.md`
This could optionally have under bar separated filename `01a03d20-a54c-7977-a1f4-1a88b38855dd_jot_that_down.md` for users to recognize with the filename when checking file names from a file explorer.

When a user wants to create a note, it will show inputs for `content` and optional `title`.
The title will be stored in the YAML frontmatter, and the content will be the markdown text.

The overall interface will look similar to those micro-blogs like twitter or mastodon.
So, I want to represent a thread and quote relation between those notes.

The sqlite database will store its title, data create, date edited, and date deleted as well as the thread/quote relation.

I think it is easy to represent the quote relation as a only single note can be selected for a note to quote that.
I wonder what the best way for a thread, but I'm considering to have an extra normalized table for a note.
