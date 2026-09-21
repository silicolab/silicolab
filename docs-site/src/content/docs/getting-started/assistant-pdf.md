---
title: Read PDFs with the Assistant
description: Let the Assistant read a paper, SI, or manual to pull out methods, parameters, and data.
sidebar:
  order: 7
---

The Assistant can read a PDF — a paper, its supporting information, or a
software manual — and use what it finds, for example to reproduce the level of
theory or the MD settings a paper reports. It works with every model provider:
Claude and GPT models receive an attached PDF itself, and every other model
receives text that SilicoLab extracts locally.

## Ask for a PDF

Drop the PDF onto the SilicoLab window, or click the paperclip in the Assistant's
message box and choose it. The PDF appears as a card in the message box — hover it
and click the × to remove it — and you can attach up to five. Say what you want
from it and send. You can also type a path yourself:

```text
Read refs/zhang2024.pdf and tell me which functional, basis set, and
dispersion correction they used for the geometry optimizations.
```

An attached PDF stays on the message as a card after you send it, and is saved
with the conversation by reference: SilicoLab stores where the file is, not a copy
of it. If you later move or delete the file, the conversation still opens and the
Assistant is told the attachment is gone.

When you type a path instead, the read runs in the background. The Assistant
answers once the text arrives, and a **Background PDF read** notice in the
conversation marks when that happens.

Long documents are read a few pages at a time. For a long paper or an SI, ask a
targeted question: the Assistant can search the document for a term first and
then read only the pages that mention it. Ask it to cite page numbers so you can
check a value against the source.

## What the model receives

| Model | An attached PDF arrives as |
| --- | --- |
| Anthropic (Claude), OpenAI (GPT) — on the provider's own endpoint | The PDF itself, so the model also sees figures, image tables, and scanned pages. |
| Gemini, DeepSeek, GLM, OpenRouter, local and custom endpoints, or any provider with an overridden base URL | Text extracted on your computer. A one-time note in the conversation says so. |
| Claude CLI, Codex CLI | The file's path; the CLI reads it from disk itself. |

Sending the PDF itself costs more. It is uploaded again with every request of the
conversation, and Claude counts roughly 1,500–3,000 tokens per page; on Claude,
prompt caching makes the follow-up requests of a turn much cheaper. To keep this
bounded, only the two most recent messages with attachments are sent in full.
Older attachments shrink to a reference, and the Assistant re-reads pages from
them on demand.

A PDF over the provider's limit — 100 pages or about 22 MB per request for
Claude, about 36 MB for GPT — is sent as extracted text instead, and the model is
told why.

## Where the file is matters

| Path | Behaviour |
| --- | --- |
| Any file you chose yourself — attached, or typed into your message | Read without asking. |
| Inside the project folder, given relative to it (`refs/paper.pdf`) | Read without asking. |
| Anywhere else the Assistant chooses on its own (an absolute path, or one containing `..`) | Shown as an **outside-project read** that you approve first in **Manual** and **Auto (safe)** modes. **Auto** reads it without asking. |

The PDF, or its extracted text, is sent to your model provider like the rest of
the conversation. Keep papers you work from in the project folder, and do not point
the Assistant at documents you would not paste into the chat yourself.

## Limits

These apply whenever the model receives extracted text rather than the PDF itself:

- Scanned PDFs have no text layer, so nothing can be read from them; there is no OCR.
- Figures, and tables that are images, are not visible to the model. Text tables
  come through as plain text and may lose their column alignment — check
  extracted numbers against the PDF.

And these apply always:

- Password-protected PDFs cannot be opened.
- Files over 64 MB or 2000 pages are refused.
