"""Phase 19.C — Python facade smoke for `ContentPart::Image`.

The Anthropic + OpenAI Rust adapters now emit outbound vision
content blocks (Phase 19.A `b67504b`, 19.B `e85953f`). This file
verifies the Python-side ergonomics:

- `tako.ContentPart` (the Pydantic v2 mirror of the core type)
  accepts the four image-content fields shipped in
  ``crates/tako-core/src/types.rs`` (`type="image"`, `mime`,
  `data_b64`).
- The model serialises to a dict matching the Rust-side wire
  shape that ``content_to_blocks`` and ``message_to_oa`` consume.
- The four MIME types Anthropic + OpenAI both accept round-trip
  cleanly.

End-to-end wire tests (wheel → wiremock → captured request body)
remain the source of truth on the Rust side — see
``crates/tako-providers/{anthropic,openai}/src/convert.rs``. This
file pins only the Python ergonomics so a regression in the
Pydantic model surface lands here before it lands in user code.
"""

from __future__ import annotations

import pytest
from tako import ContentPart, Message, Role


def test_content_part_accepts_image_variant() -> None:
    cp = ContentPart(
        type="image",
        mime="image/png",
        data_b64="aGVsbG8=",
    )
    assert cp.type == "image"
    assert cp.mime == "image/png"
    assert cp.data_b64 == "aGVsbG8="
    # Phase 1 fields default to None on an image part.
    assert cp.text is None
    assert cp.id is None


def test_content_part_image_serialises_to_expected_dict() -> None:
    cp = ContentPart(
        type="image",
        mime="image/jpeg",
        data_b64="YWJjZA==",
    )
    payload = cp.model_dump(exclude_none=True, exclude_defaults=True)
    # The wire-shape the Rust side consumes — `type` discriminator
    # plus the two image-bearing fields. `is_error` defaults to
    # False so it's excluded above.
    assert payload == {
        "type": "image",
        "mime": "image/jpeg",
        "data_b64": "YWJjZA==",
    }


def test_message_can_carry_mixed_text_and_image_content() -> None:
    """Phase 19.A / 19.B cadence — a message with text + image
    parts in source order is what `content_to_blocks` (Anthropic)
    and `message_to_oa` (OpenAI) consume to emit array-shaped
    content blocks.
    """
    msg = Message(
        role=Role.USER,
        content=[
            ContentPart(type="text", text="describe this"),
            ContentPart(type="image", mime="image/png", data_b64="aGVsbG8="),
        ],
    )
    assert len(msg.content) == 2
    assert msg.content[0].type == "text"
    assert msg.content[1].type == "image"
    # Source order is preserved through Pydantic — the Rust side
    # walks the list in this order to emit the array-shaped
    # `content` field on OpenAI / `image` block on Anthropic.


@pytest.mark.parametrize(
    "mime",
    ["image/jpeg", "image/png", "image/gif", "image/webp"],
)
def test_content_part_accepts_all_supported_mimes(mime: str) -> None:
    """Phase 19 supports the four MIME types Anthropic + OpenAI
    both accept (`is_supported_anthropic_mime` / `is_supported_openai_mime`
    on the Rust side). The Pydantic model accepts the strings
    verbatim — actual wire-side filtering happens in the Rust
    adapter.
    """
    cp = ContentPart(type="image", mime=mime, data_b64="YWI=")
    assert cp.mime == mime
