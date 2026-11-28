import type { Metadata } from "next";

import { Shell } from "@/components/shell";

import "./globals.css";

export const metadata: Metadata = {
  title: "dsync fleet",
  description: "Локальная панель управления флотом dsync",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="ru" className="dark">
      <body>
        <Shell>{children}</Shell>
      </body>
    </html>
  );
}
