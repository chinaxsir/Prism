/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        surface: {
          DEFAULT: "#0f1117",
          card: "#171a23",
          hover: "#1f2330",
        },
        accent: {
          DEFAULT: "#6366f1",
          hover: "#818cf8",
        },
        latency: {
          good: "#22c55e",
          medium: "#eab308",
          bad: "#ef4444",
        },
      },
    },
  },
  plugins: [],
};
