"use client";

import { useEffect, useState } from "react";

const KEY = "dsync.refreshSeconds";

export function useRefreshInterval(): [number, (n: number) => void] {
  const [sec, setSec] = useState(10);

  useEffect(() => {
    const id = setTimeout(() => {
      const raw = Number(window.localStorage.getItem(KEY));
      if (Number.isFinite(raw) && raw >= 5 && raw <= 60) setSec(raw);
    }, 0);
    return () => clearTimeout(id);
  }, []);

  const update = (n: number) => {
    const clamped = Math.min(60, Math.max(5, n));
    setSec(clamped);
    window.localStorage.setItem(KEY, String(clamped));
  };

  return [sec, update];
}
