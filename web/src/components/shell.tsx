"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import {
  Activity,
  Boxes,
  Cpu,
  LayoutDashboard,
  Server,
  Settings,
  Stethoscope,
} from "lucide-react";

import { cn } from "@/lib/utils";

const NAV = [
  { href: "/", label: "Dashboard", icon: LayoutDashboard },
  { href: "/machines", label: "Machines", icon: Server },
  { href: "/projects", label: "Projects", icon: Boxes },
  { href: "/state", label: "State", icon: Cpu },
  { href: "/doctor", label: "Doctor", icon: Stethoscope },
  { href: "/activity", label: "Activity", icon: Activity },
  { href: "/settings", label: "Settings", icon: Settings },
];

export function Shell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();

  return (
    <div className="flex min-h-screen">
      <aside className="fixed inset-y-0 left-0 flex w-56 flex-col border-r border-slate-800 bg-slate-950/80">
        <div className="border-b border-slate-800 px-4 py-4">
          <p className="text-sm font-semibold tracking-wide text-slate-100">dsync fleet</p>
          <p className="mt-1 text-xs text-slate-500">локальная панель</p>
        </div>
        <nav className="flex flex-1 flex-col gap-1 p-2">
          {NAV.map((item) => {
            const Icon = item.icon;
            const active =
              item.href === "/" ? pathname === "/" : pathname.startsWith(item.href);
            return (
              <Link
                key={item.href}
                href={item.href}
                className={cn(
                  "flex items-center gap-2 rounded-md px-3 py-2 text-sm text-slate-400 transition-colors hover:bg-slate-800 hover:text-slate-100",
                  active && "bg-slate-800 text-slate-100",
                )}
              >
                <Icon className="h-4 w-4" />
                {item.label}
              </Link>
            );
          })}
        </nav>
        <div className="border-t border-slate-800 p-3 text-[11px] leading-relaxed text-slate-500">
          127.0.0.1:3133
          <br />
          данные: локальный dsync CLI
        </div>
      </aside>
      <main className="ml-56 min-h-screen flex-1 px-6 py-6">{children}</main>
    </div>
  );
}
