#!/bin/sh
set -eu

# Verifies the search and delivery surface of a deployed site build.
#
# Preview deployments sit behind Vercel authentication, so an anonymous request
# returns the sign-in redirect instead of the page. When that happens this
# script reads the project's automation bypass secret and sends it as a request
# header, which leaves preview protection in place for everyone else.
#
# The secret comes from $VERCEL_AUTOMATION_BYPASS_SECRET when set, and otherwise
# from the signed-in Vercel CLI. It is never printed.
#
# Usage:
#   site/scripts/verify-deployment.sh                       # production
#   site/scripts/verify-deployment.sh <deployment-url>      # preview

target=${1:-https://www.rucksack.wtf}
target=${target%/}
vercel_project=rucksack
vercel_scope=thirdface
failures=0

report() {
    if [ "$1" = pass ]; then
        printf 'pass  %s\n' "$2"
    else
        printf 'FAIL  %s\n' "$2"
        failures=$((failures + 1))
    fi
}

# A protected deployment answers anonymous requests with a redirect to Vercel's
# sign-in endpoint rather than the page itself.
redirect_target=$(/usr/bin/curl --silent --show-error --proto '=https' --output /dev/null --write-out '%{redirect_url}' "$target/" || true)
bypass_secret=""
case $redirect_target in
    *vercel.com/sso-api*)
        bypass_secret=${VERCEL_AUTOMATION_BYPASS_SECRET:-}
        if [ -z "$bypass_secret" ]; then
            bypass_secret=$(vercel api "/v9/projects/$vercel_project" --scope "$vercel_scope" 2>/dev/null |
                sed -n '/"protectionBypass"/,/}/p' |
                sed -n 's/.*"\([A-Za-z0-9]\{32\}\)".*/\1/p' |
                head -1)
        fi
        if [ -z "$bypass_secret" ]; then
            printf 'deployment is protected and no bypass secret is available.\n'
            printf 'run `vercel login`, or set VERCEL_AUTOMATION_BYPASS_SECRET.\n'
            exit 1
        fi
        printf 'deployment is protected, using the automation bypass\n'
        ;;
esac

fetch() {
    request_path=$1
    shift
    if [ -n "$bypass_secret" ]; then
        /usr/bin/curl --silent --show-error --location --proto '=https' \
            --header "x-vercel-protection-bypass: $bypass_secret" \
            "$@" "$target$request_path" || true
    else
        /usr/bin/curl --silent --show-error --location --proto '=https' "$@" "$target$request_path" || true
    fi
}

expect_contains() {
    case $1 in
        *"$2"*) report pass "$3" ;;
        *) report fail "$3" ;;
    esac
}

printf 'verifying %s\n\n' "$target"

home_headers=$(fetch / --head)
expect_contains "$home_headers" '200' 'home page responds 200'
expect_contains "$home_headers" 'content-security-policy:' 'content security policy header present'
expect_contains "$home_headers" 'x-content-type-options: nosniff' 'nosniff header present'
expect_contains "$home_headers" 'referrer-policy:' 'referrer policy header present'

home_body=$(fetch /)
expect_contains "$home_body" '<link rel="canonical" href="https://www.rucksack.wtf/"' 'canonical points at the www origin'
expect_contains "$home_body" 'og:image' 'social preview declared'
expect_contains "$home_body" 'application/ld+json' 'structured data present'

robots=$(fetch /robots.txt)
expect_contains "$robots" 'Sitemap: https://www.rucksack.wtf/sitemap.xml' 'robots.txt advertises the sitemap'

sitemap=$(fetch /sitemap.xml)
expect_contains "$sitemap" '<loc>https://www.rucksack.wtf/</loc>' 'sitemap lists the canonical home page'

social=$(fetch /rucksack-ai-coding-agent-phone-hotspot.jpg --head)
expect_contains "$social" 'content-type: image/jpeg' 'social preview image is served as jpeg'

missing=$(fetch /this-path-does-not-exist)
expect_contains "$missing" 'noindex' 'not-found page stays out of search'

printf '\n'
if [ "$failures" -eq 0 ]; then
    printf 'all checks passed\n'
else
    printf '%s check(s) failed\n' "$failures"
    exit 1
fi
