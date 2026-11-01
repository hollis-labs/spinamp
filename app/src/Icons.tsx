import type { ReactNode } from "react";

const I = ({ children, size = 18 }: { children: ReactNode; size?: number }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden>{children}</svg>
);
const S = ({ children, size = 18 }: { children: ReactNode; size?: number }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="square" aria-hidden>{children}</svg>
);

export const Play = () => <I><path d="M7 4v16l13-8z" /></I>;
export const Pause = () => <I><path d="M6 4h4v16H6zM14 4h4v16h-4z" /></I>;
export const Next = () => <I><path d="M5 4v16l11-8zM17 4h3v16h-3z" /></I>;
export const Up = () => <S><path d="M12 19V6M6 12l6-6 6 6" /></S>;
export const Down = () => <S><path d="M12 5v13M6 12l6 6 6-6" /></S>;
export const Back = () => <S><path d="M19 12H6M11 6l-6 6 6 6" /></S>;
export const Chevron = () => <S size={12}><path d="M6 9l6 6 6-6" /></S>;
export const Dir = ({ desc }: { desc: boolean }) => <S size={14}><path d={desc ? "M12 5v14M7 14l5 5 5-5" : "M12 19V5M7 10l5-5 5 5"} /></S>;
export const Shuffle = () => <S><path d="M3 7h4l10 10h4M3 17h4l3-3M14 10l3-3h4M18 4l3 3-3 3M18 14l3 3-3 3" /></S>;
export const Spark = () => <S><path d="M12 3v5M12 16v5M3 12h5M16 12h5M6 6l3 3M15 15l3 3M18 6l-3 3M9 15l-3 3" /></S>;
export const Rows = () => <S><path d="M4 6h16M4 12h16M4 18h16" /></S>;
export const Plug = () => <S><path d="M9 3v5M15 3v5M6 8h12v4a6 6 0 0 1-12 0zM12 18v3" /></S>;
export const Plus = () => <S size={16}><path d="M12 5v14M5 12h14" /></S>;
export const Close = () => <S size={14}><path d="M6 6l12 12M18 6L6 18" /></S>;
export const Minus = () => <S size={16}><path d="M5 12h14" /></S>;
export const Gear = () => <S><path d="M4 7h10M18 7h2M4 17h2M10 17h10M14 4v6M8 14v6" /></S>;
export const Lyr = () => <S><path d="M4 6h16M4 11h10M4 16h16M4 21h7" /></S>;
export const User = () => <S><path d="M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21c0-4.4 3.6-7 8-7s8 2.6 8 7" /></S>;
export const Ext = () => <S size={12}><path d="M14 4h6v6M20 4l-9 9M18 14v5H5V6h5" /></S>;
export const More = () => <I><circle cx="5" cy="12" r="1.8" /><circle cx="12" cy="12" r="1.8" /><circle cx="19" cy="12" r="1.8" /></I>;
export const Star = ({ on }: { on: boolean }) => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill={on ? "currentColor" : "none"} stroke="currentColor" strokeWidth="2" strokeLinejoin="round" aria-hidden>
    <path d="M12 3l2.7 5.8 6.3.7-4.7 4.3 1.3 6.2L12 17l-5.6 3 1.3-6.2L3 9.5l6.3-.7z" />
  </svg>
);
export const Columns = () => <S><path d="M4 5h16v14H4zM9.5 5v14M14.5 5v14" /></S>;
export const Disc = () => <S><path d="M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z" /></S>;
export const Trash = () => <S><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" /></S>;
