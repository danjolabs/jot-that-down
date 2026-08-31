title: A file with no fence
relation:root: 01a03d53-1de8-70c1-8f16-8a5a6f6a7f10

This file has no leading `---` fence at all — it looks like frontmatter but was never delimited.
The block is always present in a well-formed note, so this is a malformed note rather than an
untitled one, and it must be rejected rather than guessed at.
