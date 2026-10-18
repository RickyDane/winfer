From: Tom Achebe
Subject: [pacel] Reusing stream ids after RESET

Long-lived connections run out of 31-bit ids eventually. Can we reuse ids after RESET like draft-01?

---
From: Mara Lind (editor)
Re: [pacel] Reusing stream ids after RESET

No. Late frames for an old stream would be delivered to the new one. 6.2 stays as is; open a new
connection when ids run out.
