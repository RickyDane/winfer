From: Ines Duarte
Subject: [pacel] Queued DATA when closing

If an application calls close() while DATA is queued, should we send it after CLOSE?

---
From: Mara Lind (editor)
Re: [pacel] Queued DATA when closing

Send it before the CLOSE frame or drop it. Nothing may follow CLOSE (7.2).
