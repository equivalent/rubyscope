// rubyscope web page: the header line and the docs contents.

const head = document.querySelector(".site-head");
const onScroll = () => head.classList.toggle("scrolled", window.scrollY > 8);
window.addEventListener("scroll", onScroll, { passive: true });
onScroll();

// Docs: mark the section that is on screen in the contents.
const toc = document.querySelector(".toc");
if (toc) {
  const links = new Map([...toc.querySelectorAll("a")].map((a) => [a.hash.slice(1), a]));
  const seen = new Set();
  const observer = new IntersectionObserver(
    (entries) => {
      entries.forEach((e) => (e.isIntersecting ? seen.add(e.target.id) : seen.delete(e.target.id)));
      const first = [...links.keys()].find((id) => seen.has(id));
      if (!first) return;
      links.forEach((a, id) => a.classList.toggle("current", id === first));
    },
    { rootMargin: "-80px 0px -60% 0px" },
  );
  links.forEach((_, id) => {
    const section = document.getElementById(id);
    if (section) observer.observe(section);
  });
}

// Landing page: the features row. Arrows, the count, and the list of names.
(() => {
  const reel = document.querySelector(".reel");
  if (!reel) return;
  const cards = [...reel.querySelectorAll(".card")];
  const names = [...document.querySelectorAll(".reel-index button")];
  const prev = document.querySelector(".reel-prev");
  const next = document.querySelector(".reel-next");
  const now = document.querySelector(".reel-now");
  const smooth = matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";
  let current = 0;

  const go = (i) => {
    i = Math.max(0, Math.min(cards.length - 1, i));
    const pad = parseFloat(getComputedStyle(reel).scrollPaddingLeft) || 0;
    reel.scrollTo({ left: cards[i].offsetLeft - pad, behavior: smooth });
  };

  const show = (i) => {
    current = i;
    now.textContent = i + 1;
    prev.disabled = i === 0;
    next.disabled = i === cards.length - 1;
    names.forEach((b, n) => b.setAttribute("aria-current", n === i ? "true" : "false"));
    const b = names[i];
    const list = b.parentElement.parentElement;
    if (list.scrollWidth > list.clientWidth) {
      list.scrollTo({ left: b.parentElement.offsetLeft - 16, behavior: smooth });
    }
  };

  // The card whose left edge is nearest the start of the row is the current one.
  const onScroll = () => {
    const pad = parseFloat(getComputedStyle(reel).scrollPaddingLeft) || 0;
    const x = reel.scrollLeft + pad;
    let best = 0;
    cards.forEach((c, i) => {
      if (Math.abs(c.offsetLeft - x) < Math.abs(cards[best].offsetLeft - x)) best = i;
    });
    if (reel.scrollLeft + reel.clientWidth >= reel.scrollWidth - 4) best = cards.length - 1;
    if (best !== current) show(best);
  };

  let t;
  reel.addEventListener("scroll", () => { clearTimeout(t); t = setTimeout(onScroll, 60); }, { passive: true });
  prev.addEventListener("click", () => go(current - 1));
  next.addEventListener("click", () => go(current + 1));
  names.forEach((b, i) => b.addEventListener("click", () => go(i)));
  reel.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight") { e.preventDefault(); go(current + 1); }
    if (e.key === "ArrowLeft") { e.preventDefault(); go(current - 1); }
  });
  show(0);
})();

// Drop the fade at the bottom of a card once it is scrolled to its end.
document.querySelectorAll(".card").forEach((card) => {
  const check = () => card.classList.toggle("at-end", card.scrollTop + card.clientHeight >= card.scrollHeight - 4);
  card.addEventListener("scroll", check, { passive: true });
  window.addEventListener("resize", check);
  card.querySelectorAll("img").forEach((img) => img.addEventListener("load", check));
  check();
});
