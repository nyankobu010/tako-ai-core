"""Phase 22.D — Python facade smoke for `ContentPart::ImageUrl`.

Phase 22 added URL-source images on the Rust side: Anthropic
(22.B) emits `AnImageSource::Url { url }`, OpenAI + Mistral
(22.C) pass URLs directly to `image_url.url`. Vertex / Bedrock /
Ollama silent-drop pending Phase 23+.

This file pins the Python-side ergonomics of the new
`ContentPart` shape so a regression in the Pydantic mirror's
`url` field surface lands here before user code.

End-to-end wire tests (wheel → wiremock → captured request body)
remain the source of truth on the Rust side — see the per-crate
unit tests in `crates/tako-providers/{anthropic,openai,mistral}/src/convert.rs`.
"""

from __future__ import annotations

import pytest

from tako import ContentPart, Message, Role


def test_content_part_accepts_image_url_variant() -> None:
    cp = ContentPart(
        type="image_url",
        url="https://example.com/cat.jpg",
        mime=None,
    )
    assert cp.type == "image_url"
    assert cp.url == "https://example.com/cat.jpg"
    assert cp.mime is None
    # Phase 1 fields default to None on a URL-source part.
    assert cp.text is None
    assert cp.data_b64 is None
    assert cp.id is None


def test_content_part_image_url_serialises_to_expected_dict() -> None:
    cp = ContentPart(
        type="image_url",
        url="https://example.com/dog.png",
    )
    payload = cp.model_dump(exclude_none=True, exclude_defaults=True)
    # The wire-shape the Rust side consumes — `type` discriminator
    # plus the `url` field. `is_error` defaults to False so excluded.
    assert payload == {
        "type": "image_url",
        "url": "https://example.com/dog.png",
    }


def test_content_part_image_url_with_optional_mime_hint() -> None:
    """Phase 22's `ContentPart::ImageUrl` carries an optional mime
    hint. The Rust adapters intentionally drop the hint (none of
    Anthropic / OpenAI / Mistral accept a `mime` field on
    URL-source content blocks), but the Pydantic model still
    accepts it for forward-compatibility — a future provider may
    use it.
    """
    cp = ContentPart(
        type="image_url",
        url="https://example.com/cat.jpg",
        mime="image/jpeg",
    )
    assert cp.mime == "image/jpeg"
    payload = cp.model_dump(exclude_none=True, exclude_defaults=True)
    assert payload == {
        "type": "image_url",
        "url": "https://example.com/cat.jpg",
        "mime": "image/jpeg",
    }


def test_message_can_carry_mixed_text_and_image_url() -> None:
    """Source order is preserved through the Pydantic round trip —
    the Rust adapters walk the list in this order to emit the
    array-shaped `content` field on OpenAI / Mistral, the
    typed-block array on Anthropic.
    """
    msg = Message(
        role=Role.USER,
        content=[
            ContentPart(type="text", text="describe this"),
            ContentPart(type="image_url", url="https://example.com/cat.jpg"),
        ],
    )
    assert len(msg.content) == 2
    assert msg.content[0].type == "text"
    assert msg.content[1].type == "image_url"
    assert msg.content[1].url == "https://example.com/cat.jpg"


@pytest.mark.parametrize(
    "url",
    [
        "https://example.com/cat.jpg",
        "https://cdn.example.org/path/to/image.png?w=1024&h=768",
        "https://images.example.net/with-query#frag",
    ],
)
def test_content_part_accepts_various_https_urls(url: str) -> None:
    """Phase 22 emits whatever URL the caller provides — vendor-
    side validation handles `http://` rejection and the rest. The
    Pydantic model accepts the strings verbatim.
    """
    cp = ContentPart(type="image_url", url=url)
    assert cp.url == url


def test_image_url_and_image_b64_can_coexist_in_one_message() -> None:
    """Phase 22 supports mixing inline-base64 and URL-source
    images in a single message. Pinned on the Rust side by
    `image_url_and_base64_can_coexist_in_one_message` (Anthropic).
    """
    msg = Message(
        role=Role.USER,
        content=[
            ContentPart(type="image", mime="image/png", data_b64="aGVsbG8="),
            ContentPart(type="image_url", url="https://example.com/dog.jpg"),
        ],
    )
    assert msg.content[0].type == "image"
    assert msg.content[0].data_b64 == "aGVsbG8="
    assert msg.content[1].type == "image_url"
    assert msg.content[1].url == "https://example.com/dog.jpg"
