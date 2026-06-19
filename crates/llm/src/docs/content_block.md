A block of content within a [`ChatMessage::User`] message.

Supports multimodal input: text, images (base64-encoded), and audio (base64-encoded).

# Variants

- **`Text`** -- Plain text content. Construct with [`ContentBlock::text("hello")`](ContentBlock::text).
- **`Image`** -- Base64-encoded image with its MIME type (e.g. `image/png`).
- **`Audio`** -- Base64-encoded audio with its MIME type (e.g. `audio/wav`).

# Working with text

- [`text()`](ContentBlock::text) -- Create a `Text` block from anything that implements `Into<String>`.
- [`first_text(parts)`](ContentBlock::first_text) -- Find the first non-empty text block in a slice.
- [`join_text(parts)`](ContentBlock::join_text) -- Concatenate all text blocks with newlines.

# Working with media

- [`is_image()`](ContentBlock::is_image) -- Check if this is an image block.
- [`as_data_uri()`](ContentBlock::as_data_uri) -- Convert image/audio blocks to a `data:{mime};base64,{data}` URI. Returns `None` for text blocks.
- [`estimated_bytes()`](ContentBlock::estimated_bytes) -- Byte-size estimate (text length or base64 data length).
