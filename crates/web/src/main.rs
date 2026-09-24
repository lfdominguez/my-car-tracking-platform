// `i18n` stays first so its `tr!` macro is in scope for every module below.
#[macro_use]
mod i18n;

mod api;
mod components;
mod default_car;
mod pages;
mod units;
mod vault;

use leptos::prelude::*;
use leptos_router::components::{ParentRoute, Route, Router, Routes};
use leptos_router::path;

use crate::components::layout::AppLayout;
use crate::components::provide_theme;
use crate::pages::cars::{CarDetailPage, CarsPage};
use crate::pages::chat::{ChatConversationPage, ChatPage};
use crate::pages::compare::TripComparePage;
use crate::pages::dashboard::DashboardPage;
use crate::pages::landing::LandingPage;
use crate::pages::login::LoginPage;
use crate::pages::not_found::NotFoundPage;
use crate::pages::notifications::NotificationsPage;
use crate::pages::places::PlacesPage;
use crate::pages::routes::{RouteCorridorPage, RoutesPage};
use crate::pages::settings::SettingsPage;
use crate::pages::stats::StatsPage;
use crate::pages::trips::{TripDetailPage, TripsPage};
use crate::vault::provide_vault_session;

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| {
        crate::i18n::provide_locale();
        provide_vault_session();
        provide_theme();
        view! {
            <Router>
                <Routes fallback=|| view! { <NotFoundPage/> }>
                    <Route path=path!("/") view=LandingPage/>
                    <Route path=path!("/login") view=LoginPage/>
                    // Keep the shell mounted across authenticated pages so the Google
                    // avatar is not re-requested on every navigation (can trigger 429).
                    <ParentRoute path=path!("/app") view=AppLayout>
                        <Route path=path!("") view=DashboardPage/>
                        <Route path=path!("cars") view=CarsPage/>
                        <Route path=path!("cars/:id") view=CarDetailPage/>
                        <Route path=path!("chat") view=ChatPage/>
                        <Route path=path!("chat/:id") view=ChatConversationPage/>
                        <Route path=path!("trips") view=TripsPage/>
                        <Route path=path!("trips/compare") view=TripComparePage/>
                        <Route path=path!("trips/:id") view=TripDetailPage/>
                        <Route path=path!("routes") view=RoutesPage/>
                        <Route path=path!("routes/:id") view=RouteCorridorPage/>
                        <Route path=path!("stats") view=StatsPage/>
                        <Route path=path!("places") view=PlacesPage/>
                        <Route path=path!("settings") view=SettingsPage/>
                        <Route path=path!("notifications") view=NotificationsPage/>
                    </ParentRoute>
                </Routes>
            </Router>
        }
    });
}
