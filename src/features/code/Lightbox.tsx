import { useEffect } from "react";
import { ChevronLeft, ChevronRight, Download, X } from "lucide-react";
import { useCode } from "./store";

/** Full-size preview of pasted images (composer thumbnails and the ones in your messages). */
export function Lightbox() {
  const preview = useCode((s) => s.preview);
  const setPreview = useCode((s) => s.setPreview);
  useEffect(() => {
    if (!preview) return;
    const on = (e: KeyboardEvent) => {
      const n = preview.images.length;
      if (e.key === "Escape") setPreview(null);
      else if (e.key === "ArrowRight" && n > 1) setPreview({ ...preview, index: (preview.index + 1) % n });
      else if (e.key === "ArrowLeft" && n > 1) setPreview({ ...preview, index: (preview.index - 1 + n) % n });
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", on, true);
    return () => window.removeEventListener("keydown", on, true);
  }, [preview, setPreview]);
  if (!preview) return null;
  const { images, index } = preview;
  const n = images.length;
  const go = (d: number) => setPreview({ images, index: (index + d + n) % n });
  return (
    <div className="cw-lightbox" onMouseDown={(e) => e.target === e.currentTarget && setPreview(null)}>
      <img key={images[index]} src={images[index]} alt="" className="cw-lightbox-img" />
      <div className="cw-lightbox-bar">
        {n > 1 && (
          <>
            <button className="cw-icon-btn" onClick={() => go(-1)} title="Previous (←)">
              <ChevronLeft size={16} />
            </button>
            <span>
              {index + 1} / {n}
            </span>
            <button className="cw-icon-btn" onClick={() => go(1)} title="Next (→)">
              <ChevronRight size={16} />
            </button>
          </>
        )}
        <a className="cw-icon-btn" href={images[index]} download={`image-${index + 1}.png`} title="Save">
          <Download size={15} />
        </a>
        <button className="cw-icon-btn" onClick={() => setPreview(null)} title="Close (esc)">
          <X size={16} />
        </button>
      </div>
    </div>
  );
}
