import { createBrowserRouter } from "react-router";

import Home from "@/routes/Home";
import About from "@/routes/About";

export const router = createBrowserRouter([
  { path: "/", element: <Home /> },
  { path: "/about", element: <About /> },
]);
